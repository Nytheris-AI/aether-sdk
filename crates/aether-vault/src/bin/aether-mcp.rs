use std::future::Future;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use nytheris_aether_core::{AetherError, Result};
use nytheris_aether_vault::{
    AgentId, ApiKeySecret, ControlPlaneServer, KvCacheAdapter, McpServer, ModelId, VaultConfig,
    VaultEngine, VllmLmCacheConfig, VllmLmCacheHttpAdapter,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunMode {
    StdioMcp,
    HttpOnly,
}

struct CliArgs {
    mode: RunMode,
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

    match parsed.mode {
        RunMode::HttpOnly => {
            let http = http_server.ok_or_else(|| {
                AetherError::InvalidInput("`--http-only` requires `--http-addr <addr>`".into())
            })?;
            run_http_only(engine, http, wait_for_shutdown_signal()).await
        }
        RunMode::StdioMcp => {
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
    }
}

async fn run_http_only<F>(
    engine: Arc<VaultEngine>,
    http_server: ControlPlaneServer,
    shutdown_signal: F,
) -> Result<()>
where
    F: Future<Output = Result<()>>,
{
    eprintln!(
        "aether-mcp HTTP control plane listening on http://{}",
        http_server.local_addr()
    );
    let signal_res = shutdown_signal.await;
    let http_res = http_server.shutdown().await;
    let engine_res = engine.shutdown().await;
    signal_res?;
    http_res?;
    engine_res
}

async fn wait_for_shutdown_signal() -> Result<()> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut sigterm =
            signal(SignalKind::terminate()).map_err(|err| AetherError::Permanent {
                connector: "aether-mcp",
                message: format!("failed to install SIGTERM handler: {err}"),
            })?;
        tokio::select! {
            res = tokio::signal::ctrl_c() => {
                res.map_err(|err| AetherError::Permanent {
                    connector: "aether-mcp",
                    message: format!("failed to wait for ctrl_c: {err}"),
                })?;
            }
            _ = sigterm.recv() => {}
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c()
            .await
            .map_err(|err| AetherError::Permanent {
                connector: "aether-mcp",
                message: format!("failed to wait for ctrl_c: {err}"),
            })
    }
}

fn parse_cli_args(args: &[String]) -> Result<CliArgs> {
    parse_cli_args_with_env(args, env_non_empty)
}

fn parse_cli_args_with_env<F>(args: &[String], env: F) -> Result<CliArgs>
where
    F: Fn(&str) -> Option<String>,
{
    let mut vault = env("AETHER_VAULT_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let mut db = None;
    let mut watch_ms: u64 = 2000;
    let mut http_addr_raw = env("AETHER_HTTP_ADDR");
    let mut http_only = false;
    let mut require_auth = env("AETHER_REQUIRE_AUTH")
        .is_some_and(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes"));
    let mut bootstrap_raw = env("AETHER_BOOTSTRAP_ADMIN_KEY");
    let mut api_key_raw = env("AETHER_API_KEY");
    let mut kv_endpoint = env("AETHER_KV_ENDPOINT");
    let mut kv_model_raw = env("AETHER_KV_MODEL");

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
            "--http-only" => {
                http_only = true;
                idx += 1;
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
    if http_only && http_addr.is_none() {
        return Err(AetherError::InvalidInput(
            "`--http-only` requires `--http-addr <addr>`".into(),
        ));
    }
    let mode = if http_only {
        RunMode::HttpOnly
    } else {
        RunMode::StdioMcp
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
        mode,
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;
    use tokio::sync::oneshot;
    use uuid::Uuid;

    use super::*;

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new() -> std::io::Result<Self> {
            let path = std::env::temp_dir().join(format!("aether_mcp_cli_{}", Uuid::new_v4()));
            fs::create_dir_all(&path)?;
            Ok(Self { path })
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn http_only_without_http_addr_returns_invalid_input() {
        let err = parse_cli_args(&args(&["aether-mcp", "--http-only", "--vault", "/tmp/v"]))
            .err()
            .expect("--http-only without --http-addr must return InvalidInput");
        assert!(
            matches!(err, AetherError::InvalidInput(_)),
            "expected InvalidInput, got {err:?}"
        );
    }

    #[test]
    fn http_only_with_http_addr_sets_http_only_mode() {
        let parsed = parse_cli_args(&args(&[
            "aether-mcp",
            "--vault",
            "/tmp/v",
            "--http-addr",
            "127.0.0.1:14771",
            "--http-only",
        ]))
        .expect("valid --http-only flags must succeed");

        assert_eq!(parsed.mode, RunMode::HttpOnly);
        assert_eq!(
            parsed.http_addr,
            Some("127.0.0.1:14771".parse().expect("valid addr"))
        );
    }

    #[test]
    fn http_only_accepts_aether_http_addr_from_env() {
        let parsed = parse_cli_args_with_env(&args(&["aether-mcp", "--http-only"]), |name| {
            (name == "AETHER_HTTP_ADDR").then(|| "127.0.0.1:14771".to_string())
        })
        .expect("--http-only with AETHER_HTTP_ADDR must succeed");

        assert_eq!(parsed.mode, RunMode::HttpOnly);
        assert_eq!(
            parsed.http_addr,
            Some("127.0.0.1:14771".parse().expect("valid addr"))
        );
    }

    #[test]
    fn default_args_select_stdio_mcp_mode() {
        let parsed = parse_cli_args(&args(&["aether-mcp"])).expect("default args must succeed");
        assert_eq!(parsed.mode, RunMode::StdioMcp);
        assert_eq!(parsed.http_addr, None);
    }

    #[tokio::test]
    async fn run_http_only_serves_requests_until_shutdown_signal() {
        let vault = TempDir::new().expect("create temp vault");
        let engine = Arc::new(
            VaultEngine::open(VaultConfig::in_memory(vault.path()))
                .await
                .expect("open engine"),
        );
        let addr: SocketAddr = "127.0.0.1:0".parse().expect("valid addr");
        let server = ControlPlaneServer::bind_with_auth(addr, Arc::clone(&engine), false)
            .await
            .expect("bind control plane");
        let bound_addr = server.local_addr();

        let (tx, rx) = oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            run_http_only(engine, server, async move {
                let _ = rx.await;
                Ok(())
            })
            .await
        });

        let mut stream = TcpStream::connect(bound_addr)
            .await
            .expect("connect to control plane");
        stream
            .write_all(b"GET /v1/status HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
            .await
            .expect("write HTTP request");
        let mut response = String::new();
        stream
            .read_to_string(&mut response)
            .await
            .expect("read HTTP response");
        assert!(
            response.starts_with("HTTP/1.1 200 OK"),
            "expected 200 OK while running in http-only mode, got: {response}"
        );

        tx.send(()).expect("trigger shutdown signal");
        task.await
            .expect("http-only task join")
            .expect("http-only shutdown must return Ok");
    }
}
