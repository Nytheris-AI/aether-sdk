use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use nytheris_aether_core::{AetherError, Result};
use nytheris_aether_vault::{ControlPlaneServer, McpServer, VaultConfig, VaultEngine};

struct CliArgs {
    vault_root: PathBuf,
    db_path: Option<PathBuf>,
    watch_interval: Option<Duration>,
    http_addr: Option<SocketAddr>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let parsed = parse_cli_args(&args)?;

    let mut config = VaultConfig::new(parsed.vault_root).with_watch_interval(parsed.watch_interval);
    if let Some(db) = parsed.db_path {
        config = config.with_db_path(db);
    }

    let engine = Arc::new(VaultEngine::open(config).await?);
    let http_server = match parsed.http_addr {
        Some(addr) => Some(ControlPlaneServer::bind(addr, Arc::clone(&engine)).await?),
        None => None,
    };

    let server = McpServer::new(Arc::clone(&engine));
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
    let mut http_addr_raw = std::env::var("AETHER_HTTP_ADDR")
        .ok()
        .filter(|s| !s.trim().is_empty());

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
            _ => idx += 1,
        }
    }

    let http_addr = match http_addr_raw {
        Some(raw) => Some(raw.parse::<SocketAddr>().map_err(|err| {
            AetherError::InvalidInput(format!("invalid --http-addr `{raw}`: {err}"))
        })?),
        None => None,
    };

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
    })
}
