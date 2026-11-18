use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use nytheris_aether_core::Result;
use nytheris_aether_vault::{McpServer, VaultConfig, VaultEngine};

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let (vault_root, db_path, watch_interval) = parse_cli_args(&args);

    let mut config = VaultConfig::new(vault_root).with_watch_interval(watch_interval);
    if let Some(db) = db_path {
        config = config.with_db_path(db);
    }

    let engine = Arc::new(VaultEngine::open(config).await?);
    let server = McpServer::new(Arc::clone(&engine));
    let serve_res = server.serve_stdio().await;
    let close_res = engine.shutdown().await;
    serve_res?;
    close_res
}

fn parse_cli_args(args: &[String]) -> (PathBuf, Option<PathBuf>, Option<Duration>) {
    let mut vault = std::env::var_os("AETHER_VAULT_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let mut db = None;
    let mut watch_ms: u64 = 2000;

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
            _ => idx += 1,
        }
    }

    let watch = if watch_ms == 0 {
        None
    } else {
        Some(Duration::from_millis(watch_ms))
    };
    (vault, db, watch)
}
