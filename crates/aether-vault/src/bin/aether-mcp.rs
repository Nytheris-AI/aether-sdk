use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use nytheris_aether_core::{AetherError, Result};
use nytheris_aether_vault::{
    AgentId, ApiKeySecret, ControlPlaneServer, KvCacheAdapter, McpServer, ModelId, VaultConfig,
    VaultEngine, VllmLmCacheConfig, VllmLmCacheHttpAdapter,
};

struct CliArgs {
    vault_root: PathBuf,
    db_path: Option<PathBuf>,
    watch_interval: Option<Duration>,
    http_addr: Option<SocketAddr>,
    require_auth: bool,
    bootstrap_admin_key: Option<ApiKeySecret>,
    api_key: Option<ApiKeySecret>,
    kv_endpoint: Option<String>,
    kv_model: Option<ModelId>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let parsed = parse_cli_args(&args)?;

    let mut config = VaultConfig::new(parsed.vault_root).with_watch_interval(parsed.watch_interval);
    if let Some(db) = parsed.db_path {
        config = config.with_db_path(db);
    }
    if let Some(endpoint) = parsed.kv_endpoint {
        let adapter: Arc<dyn KvCacheAdapter> = Arc::new(VllmLmCacheHttpAdapter::new(
            VllmLmCacheConfig::new(endpoint),
        ));
        config = config.with_kv_adapter(adapter);
    }
    if let Some(model) = parsed.kv_model {
        config = config.with_default_kv_model(model);
    }

    let engine = Arc::new(VaultEngine::open(config).await?);
    if let Some(ref secret) = parsed.bootstrap_admin_key {
        engine
            .ensure_bootstrap_admin_key(AgentId::new("bootstrap-admin")?, secret)
            .await?;
    }

    let http_server = match parsed.http_addr {
        Some(addr) => Some(
            ControlPlaneServer::bind_with_auth(addr, Arc::clone(&engine), parsed.require_auth)
                .await?,
        ),
        None => None,
    };

    let server = match parsed.api_key {
        Some(ref secret) => McpServer::with_api_key(Arc::clone(&engine), secret).await?,
        None => McpServer::new(Arc::clone(&engine)),
    };
    let serve_res = server.serve_stdio().await;
    if let Some(http) = http_server {
        let _ = http.shutdown().await;
    }
    let close_res = engine.shutdown().await;
    serve_res?;
    close_res
}

fn parse_cli_args(args: &[String]) -> Result<CliArgs> {
    let mut vault = std::env::var_os("AETHER_VAULT_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let mut db = None;
    let mut watch_ms: u64 = 2000;
    let mut http_addr_raw = env_non_empty("AETHER_HTTP_ADDR");
    let mut require_auth = env_non_empty("AETHER_REQUIRE_AUTH")
        .is_some_and(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes"));
    let mut bootstrap_raw = env_non_empty("AETHER_BOOTSTRAP_ADMIN_KEY");
    let mut api_key_raw = env_non_empty("AETHER_API_KEY");
    let mut kv_endpoint = env_non_empty("AETHER_KV_ENDPOINT");
    let mut kv_model_raw = env_non_empty("AETHER_KV_MODEL");

    let mut idx = 1usize;
    while idx < args.len() {
        match args[idx].as_str() {
            "--vault" if idx + 1 < args.len() => {
                vault = PathBuf::from(&args[idx + 1]);
                idx += 2;
            }
            "--db" if idx + 1 < args.len() => {
                db = Some(PathBuf::from(&args[idx + 1]));
                idx += 2;
            }
            "--watch-ms" if idx + 1 < args.len() => {
                watch_ms = args[idx + 1].parse().unwrap_or(2000);
                idx += 2;
            }
            "--http-addr" if idx + 1 < args.len() => {
                http_addr_raw = Some(args[idx + 1].clone());
                idx += 2;
            }
            "--require-auth" => {
                require_auth = true;
                idx += 1;
            }
            "--bootstrap-admin-key" if idx + 1 < args.len() => {
                bootstrap_raw = Some(args[idx + 1].clone());
                idx += 2;
            }
            "--api-key" if idx + 1 < args.len() => {
                api_key_raw = Some(args[idx + 1].clone());
                idx += 2;
            }
            "--kv-endpoint" if idx + 1 < args.len() => {
                kv_endpoint = Some(args[idx + 1].clone());
                idx += 2;
            }
            "--kv-model" if idx + 1 < args.len() => {
                kv_model_raw = Some(args[idx + 1].clone());
                idx += 2;
            }
            _ => idx += 1,
        }
    }

    let http_addr = match http_addr_raw {
        Some(raw) => Some(raw.parse::<SocketAddr>().map_err(|err| {
            AetherError::InvalidInput(format!("invalid --http-addr `{raw}`: {err}"))
        })?),
        None => None,
    };
    let bootstrap_admin_key = bootstrap_raw
        .as_deref()
        .map(ApiKeySecret::parse)
        .transpose()?;
    let api_key = api_key_raw
        .as_deref()
        .map(ApiKeySecret::parse)
        .transpose()?;
    let kv_model = kv_model_raw.as_deref().map(ModelId::new).transpose()?;

    let watch = if watch_ms == 0 {
        None
    } else {
        Some(Duration::from_millis(watch_ms))
    };
    Ok(CliArgs {
        vault_root: vault,
        db_path: db,
        watch_interval: watch,
        http_addr,
        require_auth,
        bootstrap_admin_key,
        api_key,
        kv_endpoint,
        kv_model,
    })
}

fn env_non_empty(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|s| !s.trim().is_empty())
}
