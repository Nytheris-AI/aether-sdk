#[path = "aether-mcp/cli.rs"]
mod cli;
#[path = "aether-mcp/setup.rs"]
mod setup;
#[path = "aether-mcp/ui.rs"]
mod ui;
#[path = "aether-mcp/uninstall.rs"]
mod uninstall;

use std::fs;
use std::future::Future;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use muninn_aether_core::{AetherError, Result};
use muninn_aether_vault::{
    AgentId, ApiKeySecret, ControlPlaneServer, KvCacheAdapter, McpServer, ModelId, VaultConfig,
    VaultEngine, VllmLmCacheConfig, VllmLmCacheHttpAdapter,
};
use serde_json::{json, Map, Value};
use uuid::Uuid;

const DEFAULT_INIT_ENDPOINT_URL: &str = "http://127.0.0.1:14771";
const OBSIDIAN_PLUGIN_ID: &str = "aether-memory";
const SOURCE_PLUGIN_DIR_NAME: &str = "obsidian-aether-plugin";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ServiceAction {
    Install,
    Uninstall,
    Status,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunMode {
    StdioMcp,
    HttpOnly,
    Init,
    Doctor,
    Setup,
    Uninstall,
    Service(ServiceAction),
}

struct CliArgs {
    mode: RunMode,
    vault_root: PathBuf,
    db_path: Option<PathBuf>,
    watch_interval: Option<Duration>,
    watch_ms_explicit: Option<u64>,
    http_addr: Option<SocketAddr>,
    require_auth: bool,
    bootstrap_admin_key: Option<ApiKeySecret>,
    api_key: Option<ApiKeySecret>,
    kv_endpoint: Option<String>,
    kv_model: Option<ModelId>,
    plugin_dir: Option<PathBuf>,
    install_clients: bool,
    vault_explicit: bool,
    assume_yes: bool,
    skip_clients: bool,
    skip_service: bool,
    purge_vault: bool,
}

#[derive(Debug, Clone)]
struct InitReport {
    resolved_vault: PathBuf,
    plugin_dir: PathBuf,
    data_json_path: PathBuf,
    endpoint_url: String,
    copied_main_js: bool,
    copied_manifest_json: bool,
    copied_styles_css: bool,
    mcp_config: Value,
    installed_clients: Vec<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    // clap prints help, version, and usage errors itself and sets the exit code.
    let raw = cli::RawCli::try_parse_from(&args).unwrap_or_else(|err| err.exit());
    if let Some(cli::Command::Completions { shell }) = raw.command {
        print!("{}", cli::completions_script(shell));
        return Ok(());
    }
    let parsed = parse_cli_args(&args)?;
    match parsed.mode {
        RunMode::Init => run_init(&parsed),
        RunMode::Doctor => run_doctor(&parsed),
        RunMode::Setup => setup::run_setup(parsed),
        RunMode::Uninstall => uninstall::run_uninstall(&parsed),
        RunMode::Service(action) => run_service(action, &parsed),
        RunMode::StdioMcp | RunMode::HttpOnly => run_server(parsed).await,
    }
}

fn run_init(parsed: &CliArgs) -> Result<()> {
    let cwd = std::env::current_dir().map_err(|err| AetherError::Permanent {
        connector: "aether-init",
        message: format!("failed to resolve current directory: {err}"),
    })?;
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("aether-mcp"));
    let mut report = init_vault_plugin(parsed, &cwd, &exe)?;
    if parsed.install_clients {
        let home = resolve_home_dir()?;
        if let Some(entry) = report
            .mcp_config
            .get("mcpServers")
            .and_then(|m| m.get("aether-vault"))
        {
            report.installed_clients =
                install_mcp_clients(&home, entry, &detected_client_names(&home))?;
        }
    }
    print_init_report(&report, parsed, &exe)
}

async fn run_server(parsed: CliArgs) -> Result<()> {
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

    if parsed.mode == RunMode::HttpOnly {
        let http = http_server.ok_or_else(|| {
            AetherError::InvalidInput("`--http-only` requires `--http-addr <addr>`".into())
        })?;
        return run_http_only(engine, http, wait_for_shutdown_signal()).await;
    }

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

fn init_vault_plugin(parsed: &CliArgs, cwd: &Path, executable: &Path) -> Result<InitReport> {
    if parsed.vault_root.exists() && !parsed.vault_root.is_dir() {
        return Err(AetherError::InvalidInput(format!(
            "vault path `{}` is not a directory",
            parsed.vault_root.display()
        )));
    }

    let plugin_dest = parsed
        .vault_root
        .join(".obsidian")
        .join("plugins")
        .join(OBSIDIAN_PLUGIN_ID);
    fs::create_dir_all(&plugin_dest).map_err(|err| AetherError::Permanent {
        connector: "aether-init",
        message: format!(
            "failed to create Obsidian plugin directory `{}`: {err}",
            plugin_dest.display()
        ),
    })?;

    let resolved_vault = fs::canonicalize(&parsed.vault_root)
        .unwrap_or_else(|_| absolutize_path(&parsed.vault_root, cwd));
    let source_dir = resolve_plugin_source_dir(parsed.plugin_dir.as_deref(), cwd);

    let copied_main_js = copy_plugin_file(source_dir.as_deref(), &plugin_dest, "main.js")?;
    let copied_manifest_json =
        copy_plugin_file(source_dir.as_deref(), &plugin_dest, "manifest.json")?;
    let copied_styles_css = copy_plugin_file(source_dir.as_deref(), &plugin_dest, "styles.css")?;

    let endpoint_url = endpoint_url_from_http_addr(parsed.http_addr);
    let data_json_path = write_plugin_data_json(
        &plugin_dest,
        &endpoint_url,
        parsed.api_key.as_ref(),
        parsed.kv_endpoint.as_deref(),
        parsed.kv_model.as_ref(),
    )?;

    let mcp_config = build_mcp_json_snippet(parsed, &resolved_vault, cwd, executable);

    Ok(InitReport {
        resolved_vault,
        plugin_dir: plugin_dest,
        data_json_path,
        endpoint_url,
        copied_main_js,
        copied_manifest_json,
        copied_styles_css,
        mcp_config,
        installed_clients: Vec::new(),
    })
}

fn endpoint_url_from_http_addr(http_addr: Option<SocketAddr>) -> String {
    match http_addr {
        None => DEFAULT_INIT_ENDPOINT_URL.to_string(),
        Some(addr) if addr.ip().is_unspecified() => match addr {
            SocketAddr::V4(v4) => format!("http://127.0.0.1:{}", v4.port()),
            SocketAddr::V6(v6) => format!("http://[::1]:{}", v6.port()),
        },
        Some(addr) => format!("http://{addr}"),
    }
}

fn absolutize_path(path: &Path, cwd: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

fn resolve_plugin_source_dir(plugin_dir_override: Option<&Path>, cwd: &Path) -> Option<PathBuf> {
    if let Some(dir) = plugin_dir_override {
        let resolved = absolutize_path(dir, cwd);
        if resolved.join("main.js").is_file() || resolved.join("manifest.json").is_file() {
            return Some(resolved);
        }
        let nested = resolved.join(SOURCE_PLUGIN_DIR_NAME);
        if nested.is_dir() {
            return Some(nested);
        }
        return resolved.is_dir().then_some(resolved);
    }

    let candidate = cwd.join(SOURCE_PLUGIN_DIR_NAME);
    candidate.is_dir().then_some(candidate)
}

fn copy_plugin_file(source_dir: Option<&Path>, dest_dir: &Path, filename: &str) -> Result<bool> {
    let Some(dir) = source_dir else {
        return Ok(false);
    };
    let src = dir.join(filename);
    if !src.is_file() {
        return Ok(false);
    }
    let dst = dest_dir.join(filename);
    if let (Ok(canon_src), Ok(canon_dst)) = (fs::canonicalize(&src), fs::canonicalize(&dst)) {
        if canon_src == canon_dst {
            return Ok(true);
        }
    }
    let bytes = fs::read(&src).map_err(|err| AetherError::Permanent {
        connector: "aether-init",
        message: format!("failed to read `{}`: {err}", src.display()),
    })?;
    write_file_atomic(&dst, &bytes)?;
    Ok(true)
}

fn write_plugin_data_json(
    plugin_dest: &Path,
    endpoint_url: &str,
    api_key: Option<&ApiKeySecret>,
    kv_endpoint: Option<&str>,
    kv_model: Option<&ModelId>,
) -> Result<PathBuf> {
    let data_path = plugin_dest.join("data.json");
    let mut settings = if data_path.is_file() {
        let raw = fs::read_to_string(&data_path).map_err(|err| AetherError::Permanent {
            connector: "aether-init",
            message: format!("failed to read `{}`: {err}", data_path.display()),
        })?;
        let parsed: Value = serde_json::from_str(&raw)?;
        match parsed {
            Value::Object(map) => map,
            _ => {
                return Err(AetherError::InvalidInput(format!(
                    "existing `{}` must be a JSON object",
                    data_path.display()
                )));
            }
        }
    } else {
        default_plugin_settings_map()
    };

    settings.insert(
        "endpointUrl".to_string(),
        Value::String(endpoint_url.to_string()),
    );
    if let Some(secret) = api_key {
        settings.insert(
            "apiKey".to_string(),
            Value::String(secret.expose_secret().to_string()),
        );
    } else if !settings.contains_key("apiKey") {
        settings.insert("apiKey".to_string(), Value::String(String::new()));
    }
    if let Some(ep) = kv_endpoint {
        settings.insert("kvEndpoint".to_string(), Value::String(ep.to_string()));
    }
    if let Some(model) = kv_model {
        settings.insert(
            "kvModel".to_string(),
            Value::String(model.as_str().to_string()),
        );
    }

    let mut rendered = serde_json::to_string_pretty(&Value::Object(settings))?;
    rendered.push('\n');
    write_file_atomic(&data_path, rendered.as_bytes())?;
    Ok(data_path)
}

fn default_plugin_settings_map() -> Map<String, Value> {
    let mut map = Map::new();
    map.insert("liveSyncEnabled".to_string(), json!(true));
    map.insert("syncDebounceMs".to_string(), json!(1500));
    map.insert("autoPullIntervalSec".to_string(), json!(0));
    map.insert("runDreamOnPush".to_string(), json!(false));
    map.insert("kvEndpoint".to_string(), json!(""));
    map.insert("kvModel".to_string(), json!(""));
    map
}

fn write_file_atomic(dest: &Path, bytes: &[u8]) -> Result<()> {
    let file_name = dest.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let tmp = dest.with_file_name(format!(".{file_name}.tmp.{}", Uuid::new_v4()));
    if let Err(err) = fs::write(&tmp, bytes) {
        let _ = fs::remove_file(&tmp);
        return Err(AetherError::Permanent {
            connector: "aether-init",
            message: format!("failed to write `{}`: {err}", tmp.display()),
        });
    }
    if let Err(err) = fs::rename(&tmp, dest) {
        let _ = fs::remove_file(&tmp);
        return Err(AetherError::Permanent {
            connector: "aether-init",
            message: format!("failed to move `{}` into place: {err}", dest.display()),
        });
    }
    Ok(())
}

fn build_mcp_json_snippet(
    parsed: &CliArgs,
    resolved_vault: &Path,
    cwd: &Path,
    executable: &Path,
) -> Value {
    let mut mcp_args = vec!["--vault".to_string(), resolved_vault.display().to_string()];
    if let Some(ref db) = parsed.db_path {
        mcp_args.push("--db".to_string());
        mcp_args.push(absolutize_path(db, cwd).display().to_string());
    }
    if let Some(ms) = parsed.watch_ms_explicit {
        mcp_args.push("--watch-ms".to_string());
        mcp_args.push(ms.to_string());
    }
    if let Some(ref ep) = parsed.kv_endpoint {
        mcp_args.push("--kv-endpoint".to_string());
        mcp_args.push(ep.clone());
    }
    if let Some(ref model) = parsed.kv_model {
        mcp_args.push("--kv-model".to_string());
        mcp_args.push(model.as_str().to_string());
    }

    let mut server = Map::new();
    server.insert(
        "command".to_string(),
        Value::String(executable.display().to_string()),
    );
    server.insert("args".to_string(), json!(mcp_args));
    if let Some(ref secret) = parsed.api_key {
        server.insert(
            "env".to_string(),
            json!({ "AETHER_API_KEY": secret.expose_secret() }),
        );
    }

    json!({
        "mcpServers": {
            "aether-vault": Value::Object(server)
        }
    })
}

fn print_init_report(report: &InitReport, parsed: &CliArgs, executable: &Path) -> Result<()> {
    eprintln!(
        "Configured Obsidian Aether plugin directory: {}",
        report.plugin_dir.display()
    );
    eprintln!(
        "  main.js: {}",
        if report.copied_main_js {
            "copied"
        } else {
            "not found (build with `cd obsidian-aether-plugin && npm run build` or set AETHER_PLUGIN_DIR)"
        }
    );
    eprintln!(
        "  manifest.json: {}",
        if report.copied_manifest_json {
            "copied"
        } else {
            "not found (copy obsidian-aether-plugin/manifest.json or set AETHER_PLUGIN_DIR)"
        }
    );
    eprintln!(
        "  styles.css: {}",
        if report.copied_styles_css {
            "copied"
        } else {
            "not found (optional)"
        }
    );
    eprintln!(
        "  data.json: {} (endpointUrl = {})",
        report.data_json_path.display(),
        report.endpoint_url
    );
    let http_bind = parsed
        .http_addr
        .map(|addr| addr.to_string())
        .unwrap_or_else(|| "127.0.0.1:14771".to_string());
    eprintln!(
        "\nTo run the standalone HTTP control plane for Obsidian, execute:\n  {} --vault {} --http-addr {} --http-only",
        executable.display(),
        report.resolved_vault.display(),
        http_bind
    );
    if !report.installed_clients.is_empty() {
        eprintln!("\nUpdated MCP client configs:");
        for path in &report.installed_clients {
            eprintln!("  {}", path.display());
        }
    }
    eprintln!("\nPaste this into mcp.json (Cursor / Claude Desktop / Antigravity):");
    let rendered = serde_json::to_string_pretty(&report.mcp_config)?;
    println!("{rendered}");
    Ok(())
}

fn home_dir_from_env<F>(env: &F) -> Option<PathBuf>
where
    F: Fn(&str) -> Option<String>,
{
    env("HOME")
        .or_else(|| env("USERPROFILE"))
        .map(PathBuf::from)
}

fn default_vault_path<F>(env: &F) -> PathBuf
where
    F: Fn(&str) -> Option<String>,
{
    home_dir_from_env(env)
        .map(|home| home.join(".aether").join("vault"))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn known_mcp_client_configs(home_dir: &Path) -> Vec<(&'static str, PathBuf, PathBuf)> {
    let mut clients = vec![
        (
            "Cursor",
            home_dir.join(".cursor"),
            home_dir.join(".cursor").join("mcp.json"),
        ),
        (
            "Claude Code",
            home_dir.join(".claude"),
            home_dir.join(".claude.json"),
        ),
        (
            "Antigravity",
            home_dir.join(".gemini").join("antigravity"),
            home_dir
                .join(".gemini")
                .join("antigravity")
                .join("mcp_config.json"),
        ),
        (
            "Claude Desktop (macOS)",
            home_dir
                .join("Library")
                .join("Application Support")
                .join("Claude"),
            home_dir
                .join("Library")
                .join("Application Support")
                .join("Claude")
                .join("claude_desktop_config.json"),
        ),
        (
            "Claude Desktop (Linux)",
            home_dir.join(".config").join("Claude"),
            home_dir
                .join(".config")
                .join("Claude")
                .join("claude_desktop_config.json"),
        ),
    ];
    if let Ok(appdata) = std::env::var("APPDATA") {
        if !appdata.trim().is_empty() {
            let claude_dir = PathBuf::from(appdata).join("Claude");
            clients.push((
                "Claude Desktop (Windows)",
                claude_dir.clone(),
                claude_dir.join("claude_desktop_config.json"),
            ));
        }
    }
    clients
}

fn merge_mcp_server_into_config(
    existing_raw: Option<&str>,
    server_name: &str,
    server_entry: &Value,
) -> Result<String> {
    let mut root = match existing_raw.map(str::trim).filter(|s| !s.is_empty()) {
        Some(raw) => {
            let parsed: Value = serde_json::from_str(raw)?;
            match parsed {
                Value::Object(map) => map,
                _ => {
                    return Err(AetherError::InvalidInput(
                        "MCP config root must be a JSON object".into(),
                    ));
                }
            }
        }
        None => Map::new(),
    };

    let mut servers = match root.remove("mcpServers") {
        Some(Value::Object(map)) => map,
        Some(_) => {
            return Err(AetherError::InvalidInput(
                "`mcpServers` in MCP config must be a JSON object".into(),
            ));
        }
        None => Map::new(),
    };
    servers.insert(server_name.to_string(), server_entry.clone());
    root.insert("mcpServers".to_string(), Value::Object(servers));

    let mut rendered = serde_json::to_string_pretty(&Value::Object(root))?;
    rendered.push('\n');
    Ok(rendered)
}

/// Names of the MCP clients that have a config file or a folder on this machine.
fn detected_client_names(home_dir: &Path) -> Vec<&'static str> {
    known_mcp_client_configs(home_dir)
        .into_iter()
        .filter(|(_, dir, file)| dir.is_dir() || file.is_file())
        .map(|(name, _, _)| name)
        .collect()
}

/// Adds the server entry to the detected clients whose names are in `selected`.
fn install_mcp_clients(
    home_dir: &Path,
    server_entry: &Value,
    selected: &[&str],
) -> Result<Vec<PathBuf>> {
    let mut updated = Vec::new();
    for (name, marker_dir, config_path) in known_mcp_client_configs(home_dir) {
        if !selected.contains(&name) {
            continue;
        }
        if !marker_dir.is_dir() && !config_path.is_file() {
            continue;
        }
        if let Some(parent) = config_path.parent() {
            fs::create_dir_all(parent).map_err(|err| AetherError::Permanent {
                connector: "aether-init",
                message: format!("failed to create `{}`: {err}", parent.display()),
            })?;
        }
        let existing = if config_path.is_file() {
            Some(
                fs::read_to_string(&config_path).map_err(|err| AetherError::Permanent {
                    connector: "aether-init",
                    message: format!("failed to read `{}`: {err}", config_path.display()),
                })?,
            )
        } else {
            None
        };
        let rendered =
            merge_mcp_server_into_config(existing.as_deref(), "aether-vault", server_entry)?;
        write_file_atomic(&config_path, rendered.as_bytes())?;
        updated.push(config_path);
    }
    Ok(updated)
}

#[derive(Debug, Clone)]
struct DoctorReport {
    vault_root: PathBuf,
    vault_exists: bool,
    sqlite_exists: bool,
    configured_clients: Vec<(&'static str, PathBuf, bool)>,
}

fn collect_doctor_report(vault_root: &Path, home_dir: Option<&Path>) -> Result<DoctorReport> {
    let vault_exists = vault_root.is_dir();
    let sqlite_exists = vault_root.join(".aether").join("aether.sqlite").is_file();
    let mut configured_clients = Vec::new();

    if let Some(home) = home_dir {
        for (name, marker_dir, config_path) in known_mcp_client_configs(home) {
            if !marker_dir.is_dir() && !config_path.is_file() {
                continue;
            }
            let has_aether = config_path
                .is_file()
                .then(|| fs::read_to_string(&config_path).ok())
                .flatten()
                .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
                .and_then(|val| val.get("mcpServers")?.get("aether-vault").cloned())
                .is_some();
            configured_clients.push((name, config_path, has_aether));
        }
    }

    Ok(DoctorReport {
        vault_root: vault_root.to_path_buf(),
        vault_exists,
        sqlite_exists,
        configured_clients,
    })
}

fn doctor_lines(report: &DoctorReport, exe: &Path) -> Vec<ui::Line> {
    use ui::{Level, Line};
    let vault = report.vault_root.display();
    let mut lines = vec![Line::new(
        Level::Ok,
        format!("aether-mcp binary: {}", exe.display()),
    )];
    lines.push(if report.vault_exists {
        Line::new(Level::Ok, format!("vault path: {vault} (ok)"))
    } else {
        Line::new(Level::Fail, format!("vault path: {vault} (missing)"))
            .hint("create the folder, or pass --vault <path> pointing at your Obsidian vault")
    });
    lines.push(if report.sqlite_exists {
        Line::new(Level::Ok, "sqlite index: ok (.aether/aether.sqlite)")
    } else {
        Line::new(Level::Info, "sqlite index: not initialized yet")
            .hint("it is created the first time aether-mcp runs on this vault")
    });
    if report.configured_clients.is_empty() {
        lines.push(
            Line::new(Level::Warn, "mcp clients: none detected").hint(
                "install Cursor, Claude Desktop, Claude Code, or Antigravity, then run `aether-mcp --init --install-clients`",
            ),
        );
    } else {
        lines.push(Line::new(Level::Info, "mcp clients:"));
        for (name, path, configured) in &report.configured_clients {
            let path = path.display();
            lines.push(if *configured {
                Line::new(Level::Ok, format!("{name}: configured ({path})")).nested()
            } else {
                Line::new(Level::Warn, format!("{name}: not configured ({path})"))
                    .nested()
                    .hint(format!(
                        "run `aether-mcp --init --install-clients --vault {vault}`"
                    ))
            });
        }
    }
    lines
}

fn run_doctor(parsed: &CliArgs) -> Result<()> {
    let cwd = std::env::current_dir().map_err(|err| AetherError::Permanent {
        connector: "aether-doctor",
        message: format!("failed to resolve current directory: {err}"),
    })?;
    let vault = absolutize_path(&parsed.vault_root, &cwd);
    let home = home_dir_from_env(&env_non_empty);
    let report = collect_doctor_report(&vault, home.as_deref())?;
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("aether-mcp"));

    let lines = doctor_lines(&report, &exe);
    let ui = ui::Ui::detect();
    ui.intro("aether-mcp doctor");
    ui.print(&lines);
    let healthy = lines.iter().all(|l| l.level != ui::Level::Fail);
    ui.outro(if healthy {
        "No blocking problems found"
    } else {
        "Fix the items marked above, then run `aether-mcp doctor` again"
    });

    if !report.vault_exists {
        return Err(AetherError::InvalidInput(format!(
            "vault directory `{}` does not exist",
            report.vault_root.display()
        )));
    }
    Ok(())
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
    let mut vault_explicit = env("AETHER_VAULT_PATH").is_some();
    let mut vault = env("AETHER_VAULT_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| default_vault_path(&env));
    let mut watch_ms: u64 = 2000;
    let mut watch_ms_explicit: Option<u64> = None;
    let mut http_addr_raw = env("AETHER_HTTP_ADDR");
    let mut require_auth = env("AETHER_REQUIRE_AUTH")
        .is_some_and(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes"));
    let mut bootstrap_raw = env("AETHER_BOOTSTRAP_ADMIN_KEY");
    let mut api_key_raw = env("AETHER_API_KEY");
    let mut kv_endpoint = env("AETHER_KV_ENDPOINT");
    let mut kv_model_raw = env("AETHER_KV_MODEL");
    let plugin_dir = env("AETHER_PLUGIN_DIR").map(PathBuf::from);

    let raw = cli::RawCli::try_parse_from(args)
        .map_err(|err| AetherError::InvalidInput(err.to_string()))?;
    if let Some(path) = raw.vault {
        vault = path;
        vault_explicit = true;
    }
    let db = raw.db;
    if let Some(ms) = raw.watch_ms {
        watch_ms = ms;
        watch_ms_explicit = Some(ms);
    }
    http_addr_raw = raw.http_addr.or(http_addr_raw);
    bootstrap_raw = raw.bootstrap_admin_key.or(bootstrap_raw);
    api_key_raw = raw.api_key.or(api_key_raw);
    kv_endpoint = raw.kv_endpoint.or(kv_endpoint);
    kv_model_raw = raw.kv_model.or(kv_model_raw);
    require_auth |= raw.require_auth;
    let (http_only, init, install_clients) = (raw.http_only, raw.init, raw.install_clients);
    let doctor = matches!(raw.command, Some(cli::Command::Doctor));
    let (setup, assume_yes, skip_clients, skip_service) = match raw.command {
        Some(cli::Command::Setup {
            yes,
            no_clients,
            no_service,
        }) => (true, yes, no_clients, no_service),
        _ => (false, false, false, false),
    };
    let uninstall_flags = match raw.command {
        Some(cli::Command::Uninstall { yes, purge_vault }) => Some((yes, purge_vault)),
        _ => None,
    };
    let uninstall = uninstall_flags.is_some();
    let (uninstall_yes, purge_vault) = uninstall_flags.unwrap_or((false, false));
    if purge_vault && !vault_explicit {
        return Err(AetherError::InvalidInput(
            "`--purge-vault` needs `--vault <path>`, so it is clear which vault to clean".into(),
        ));
    }
    let service_action = match raw.command {
        Some(cli::Command::Service { action }) => Some(match action {
            cli::ServiceCmd::Install => ServiceAction::Install,
            cli::ServiceCmd::Uninstall => ServiceAction::Uninstall,
            cli::ServiceCmd::Status => ServiceAction::Status,
        }),
        _ => None,
    };

    let http_addr = match http_addr_raw {
        Some(raw) => Some(raw.parse::<SocketAddr>().map_err(|err| {
            AetherError::InvalidInput(format!("invalid --http-addr `{raw}`: {err}"))
        })?),
        None if service_action == Some(ServiceAction::Install) => Some(
            "127.0.0.1:14771"
                .parse::<SocketAddr>()
                .expect("default loopback addr"),
        ),
        None => None,
    };
    if setup && (init || http_only) {
        return Err(AetherError::InvalidInput(
            "`setup` cannot be combined with `--init` or `--http-only`".into(),
        ));
    }
    if uninstall && (init || http_only) {
        return Err(AetherError::InvalidInput(
            "`uninstall` cannot be combined with `--init` or `--http-only`".into(),
        ));
    }
    let mode = match (doctor, service_action, init, http_only) {
        _ if setup => RunMode::Setup,
        _ if uninstall => RunMode::Uninstall,
        (true, Some(_), _, _) | (true, _, true, _) | (true, _, _, true) => {
            return Err(AetherError::InvalidInput(
                "`doctor` cannot be combined with `service`, `--init`, or `--http-only`".into(),
            ));
        }
        (true, None, false, false) => RunMode::Doctor,
        (false, Some(_), true, _) | (false, Some(_), _, true) => {
            return Err(AetherError::InvalidInput(
                "`service` cannot be combined with `--init` or `--http-only`".into(),
            ));
        }
        (false, Some(action), false, false) => RunMode::Service(action),
        (false, None, true, true) => {
            return Err(AetherError::InvalidInput(
                "`--init` and `--http-only` cannot be used together".into(),
            ));
        }
        (false, None, true, false) => RunMode::Init,
        (false, None, false, true) => {
            if http_addr.is_none() {
                return Err(AetherError::InvalidInput(
                    "`--http-only` requires `--http-addr <addr>`".into(),
                ));
            }
            RunMode::HttpOnly
        }
        (false, None, false, false) => RunMode::StdioMcp,
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
        watch_ms_explicit,
        http_addr,
        require_auth,
        bootstrap_admin_key,
        api_key,
        kv_endpoint,
        kv_model,
        plugin_dir,
        install_clients,
        vault_explicit,
        assume_yes: assume_yes || uninstall_yes,
        skip_clients,
        skip_service,
        purge_vault,
    })
}

fn env_non_empty(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|s| !s.trim().is_empty())
}

#[derive(Debug, Clone)]
struct ServiceSpec {
    exe: PathBuf,
    vault_root: PathBuf,
    http_addr: SocketAddr,
    watch_ms: u64,
}

fn escape_systemd_arg(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn render_systemd_unit(spec: &ServiceSpec) -> String {
    let exe = escape_systemd_arg(&spec.exe.display().to_string());
    let vault = escape_systemd_arg(&spec.vault_root.display().to_string());
    format!(
        "[Unit]\n\
Description=Aether MCP Obsidian Vault Control Plane\n\
After=network-online.target\n\
\n\
[Service]\n\
Type=simple\n\
ExecStart=\"{exe}\" --vault \"{vault}\" --watch-ms {} --http-addr {} --http-only\n\
Restart=on-failure\n\
RestartSec=3\n\
\n\
[Install]\n\
WantedBy=default.target\n",
        spec.watch_ms, spec.http_addr
    )
}

fn render_launchd_plist(spec: &ServiceSpec, log_dir: &Path) -> String {
    let exe = escape_xml(&spec.exe.display().to_string());
    let vault = escape_xml(&spec.vault_root.display().to_string());
    let stdout_log = escape_xml(&log_dir.join("aether-mcp.out.log").display().to_string());
    let stderr_log = escape_xml(&log_dir.join("aether-mcp.err.log").display().to_string());
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\">\n\
<dict>\n\
  <key>Label</key>\n\
  <string>hq.muninn.aether-mcp</string>\n\
  <key>ProgramArguments</key>\n\
  <array>\n\
    <string>{exe}</string>\n\
    <string>--vault</string>\n\
    <string>{vault}</string>\n\
    <string>--watch-ms</string>\n\
    <string>{}</string>\n\
    <string>--http-addr</string>\n\
    <string>{}</string>\n\
    <string>--http-only</string>\n\
  </array>\n\
  <key>RunAtLoad</key>\n\
  <true/>\n\
  <key>KeepAlive</key>\n\
  <true/>\n\
  <key>StandardOutPath</key>\n\
  <string>{stdout_log}</string>\n\
  <key>StandardErrorPath</key>\n\
  <string>{stderr_log}</string>\n\
</dict>\n\
</plist>\n",
        spec.watch_ms, spec.http_addr
    )
}

fn windows_task_args(spec: &ServiceSpec) -> Vec<String> {
    let command = format!(
        "\"{}\" --vault \"{}\" --watch-ms {} --http-addr {} --http-only",
        spec.exe.display(),
        spec.vault_root.display(),
        spec.watch_ms,
        spec.http_addr
    );
    vec![
        "/Create".into(),
        "/F".into(),
        "/TN".into(),
        "AetherMcpService".into(),
        "/SC".into(),
        "ONLOGON".into(),
        "/TR".into(),
        command,
    ]
}

fn resolve_home_dir() -> Result<PathBuf> {
    env_non_empty("HOME")
        .or_else(|| env_non_empty("USERPROFILE"))
        .map(PathBuf::from)
        .ok_or_else(|| AetherError::Permanent {
            connector: "aether-service",
            message: "could not determine user home directory (HOME/USERPROFILE unset)".into(),
        })
}

fn run_command_checked(program: &str, args: &[&str]) -> Result<std::process::Output> {
    let output = std::process::Command::new(program)
        .args(args)
        .output()
        .map_err(|err| AetherError::Permanent {
            connector: "aether-service",
            message: format!("failed to run `{program}`: {err}"),
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(AetherError::Permanent {
            connector: "aether-service",
            message: format!("`{program} {}` failed: {}", args.join(" "), stderr.trim()),
        });
    }
    Ok(output)
}

fn run_service(action: ServiceAction, parsed: &CliArgs) -> Result<()> {
    let home = resolve_home_dir()?;
    if cfg!(target_os = "linux") {
        let unit_path = home.join(".config/systemd/user/aether-mcp.service");
        match action {
            ServiceAction::Install => {
                let vault = fs::canonicalize(&parsed.vault_root).map_err(|err| {
                    AetherError::InvalidInput(format!(
                        "invalid --vault `{}`: {err}",
                        parsed.vault_root.display()
                    ))
                })?;
                let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("aether-mcp"));
                let spec = ServiceSpec {
                    exe,
                    vault_root: vault,
                    http_addr: parsed
                        .http_addr
                        .unwrap_or_else(|| "127.0.0.1:14771".parse().expect("default addr")),
                    watch_ms: parsed.watch_ms_explicit.unwrap_or(2000),
                };
                if let Some(parent) = unit_path.parent() {
                    fs::create_dir_all(parent).map_err(|err| AetherError::Permanent {
                        connector: "aether-service",
                        message: format!("failed to create {}: {err}", parent.display()),
                    })?;
                }
                fs::write(&unit_path, render_systemd_unit(&spec)).map_err(|err| {
                    AetherError::Permanent {
                        connector: "aether-service",
                        message: format!("failed to write {}: {err}", unit_path.display()),
                    }
                })?;
                run_command_checked("systemctl", &["--user", "daemon-reload"])?;
                run_command_checked(
                    "systemctl",
                    &["--user", "enable", "--now", "aether-mcp.service"],
                )?;
                ui::Ui::detect().success(&format!(
                    "Installed and started systemd user service: {}",
                    unit_path.display()
                ));
                Ok(())
            }
            ServiceAction::Uninstall => {
                let _ = std::process::Command::new("systemctl")
                    .args(["--user", "disable", "--now", "aether-mcp.service"])
                    .output();
                if unit_path.exists() {
                    fs::remove_file(&unit_path).map_err(|err| AetherError::Permanent {
                        connector: "aether-service",
                        message: format!("failed to remove {}: {err}", unit_path.display()),
                    })?;
                    let _ = std::process::Command::new("systemctl")
                        .args(["--user", "daemon-reload"])
                        .output();
                }
                ui::Ui::detect().success("Uninstalled aether-mcp systemd user service.");
                Ok(())
            }
            ServiceAction::Status => {
                if !unit_path.exists() {
                    println!(
                        "aether-mcp service is not installed ({}).",
                        unit_path.display()
                    );
                    return Ok(());
                }
                let output = std::process::Command::new("systemctl")
                    .args(["--user", "status", "aether-mcp.service", "--no-pager"])
                    .output()
                    .map_err(|err| AetherError::Permanent {
                        connector: "aether-service",
                        message: format!("failed to run systemctl: {err}"),
                    })?;
                print!("{}", String::from_utf8_lossy(&output.stdout));
                if !output.status.success() {
                    eprint!("{}", String::from_utf8_lossy(&output.stderr));
                }
                Ok(())
            }
        }
    } else if cfg!(target_os = "macos") {
        let plist_path = home.join("Library/LaunchAgents/hq.muninn.aether-mcp.plist");
        let log_dir = home.join("Library/Logs/aether-mcp");
        let plist_str = plist_path.display().to_string();
        match action {
            ServiceAction::Install => {
                let vault = fs::canonicalize(&parsed.vault_root).map_err(|err| {
                    AetherError::InvalidInput(format!(
                        "invalid --vault `{}`: {err}",
                        parsed.vault_root.display()
                    ))
                })?;
                let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("aether-mcp"));
                let spec = ServiceSpec {
                    exe,
                    vault_root: vault,
                    http_addr: parsed
                        .http_addr
                        .unwrap_or_else(|| "127.0.0.1:14771".parse().expect("default addr")),
                    watch_ms: parsed.watch_ms_explicit.unwrap_or(2000),
                };
                if let Some(parent) = plist_path.parent() {
                    fs::create_dir_all(parent).map_err(|err| AetherError::Permanent {
                        connector: "aether-service",
                        message: format!("failed to create {}: {err}", parent.display()),
                    })?;
                }
                fs::create_dir_all(&log_dir).map_err(|err| AetherError::Permanent {
                    connector: "aether-service",
                    message: format!("failed to create {}: {err}", log_dir.display()),
                })?;
                fs::write(&plist_path, render_launchd_plist(&spec, &log_dir)).map_err(|err| {
                    AetherError::Permanent {
                        connector: "aether-service",
                        message: format!("failed to write {}: {err}", plist_path.display()),
                    }
                })?;
                let _ = std::process::Command::new("launchctl")
                    .args(["unload", &plist_str])
                    .output();
                run_command_checked("launchctl", &["load", "-w", &plist_str])?;
                ui::Ui::detect().success(&format!(
                    "Installed and loaded launchd agent: {}",
                    plist_path.display()
                ));
                Ok(())
            }
            ServiceAction::Uninstall => {
                let _ = std::process::Command::new("launchctl")
                    .args(["unload", "-w", &plist_str])
                    .output();
                if plist_path.exists() {
                    fs::remove_file(&plist_path).map_err(|err| AetherError::Permanent {
                        connector: "aether-service",
                        message: format!("failed to remove {}: {err}", plist_path.display()),
                    })?;
                }
                ui::Ui::detect().success("Uninstalled aether-mcp launchd agent.");
                Ok(())
            }
            ServiceAction::Status => {
                if !plist_path.exists() {
                    println!(
                        "aether-mcp service is not installed ({}).",
                        plist_path.display()
                    );
                    return Ok(());
                }
                let output = std::process::Command::new("launchctl")
                    .args(["list", "hq.muninn.aether-mcp"])
                    .output()
                    .map_err(|err| AetherError::Permanent {
                        connector: "aether-service",
                        message: format!("failed to run launchctl: {err}"),
                    })?;
                print!("{}", String::from_utf8_lossy(&output.stdout));
                if !output.status.success() {
                    eprint!("{}", String::from_utf8_lossy(&output.stderr));
                }
                Ok(())
            }
        }
    } else if cfg!(target_os = "windows") {
        match action {
            ServiceAction::Install => {
                let vault = fs::canonicalize(&parsed.vault_root).map_err(|err| {
                    AetherError::InvalidInput(format!(
                        "invalid --vault `{}`: {err}",
                        parsed.vault_root.display()
                    ))
                })?;
                let exe =
                    std::env::current_exe().unwrap_or_else(|_| PathBuf::from("aether-mcp.exe"));
                let spec = ServiceSpec {
                    exe,
                    vault_root: vault,
                    http_addr: parsed
                        .http_addr
                        .unwrap_or_else(|| "127.0.0.1:14771".parse().expect("default addr")),
                    watch_ms: parsed.watch_ms_explicit.unwrap_or(2000),
                };
                let args = windows_task_args(&spec);
                let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
                run_command_checked("schtasks", &arg_refs)?;
                run_command_checked("schtasks", &["/Run", "/TN", "AetherMcpService"])?;
                ui::Ui::detect()
                    .success("Installed and started Windows scheduled task: AetherMcpService");
                Ok(())
            }
            ServiceAction::Uninstall => {
                let _ = std::process::Command::new("schtasks")
                    .args(["/End", "/TN", "AetherMcpService"])
                    .output();
                let _ = std::process::Command::new("schtasks")
                    .args(["/Delete", "/F", "/TN", "AetherMcpService"])
                    .output();
                ui::Ui::detect().success("Uninstalled Windows scheduled task: AetherMcpService");
                Ok(())
            }
            ServiceAction::Status => {
                let output = std::process::Command::new("schtasks")
                    .args(["/Query", "/TN", "AetherMcpService", "/FO", "LIST"])
                    .output()
                    .map_err(|err| AetherError::Permanent {
                        connector: "aether-service",
                        message: format!("failed to run schtasks: {err}"),
                    })?;
                print!("{}", String::from_utf8_lossy(&output.stdout));
                if !output.status.success() {
                    eprint!("{}", String::from_utf8_lossy(&output.stderr));
                }
                Ok(())
            }
        }
    } else {
        Err(AetherError::Permanent {
            connector: "aether-service",
            message: "unsupported OS for `service` management".into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;
    use tokio::sync::oneshot;

    use super::*;

    mod tempfile {
        use std::fs;
        use std::path::{Path, PathBuf};
        use uuid::Uuid;

        pub struct TempDir {
            path: PathBuf,
        }

        impl TempDir {
            pub fn path(&self) -> &Path {
                &self.path
            }
        }

        impl Drop for TempDir {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.path);
            }
        }

        pub fn tempdir() -> std::io::Result<TempDir> {
            let path = std::env::temp_dir().join(format!("aether_mcp_cli_{}", Uuid::new_v4()));
            fs::create_dir_all(&path)?;
            Ok(TempDir { path })
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

    #[test]
    fn init_and_http_only_together_return_invalid_input() {
        let err = parse_cli_args(&args(&[
            "aether-mcp",
            "--init",
            "--http-only",
            "--http-addr",
            "127.0.0.1:14771",
        ]))
        .err()
        .expect("--init and --http-only together must return InvalidInput");
        assert!(
            matches!(err, AetherError::InvalidInput(_)),
            "expected InvalidInput, got {err:?}"
        );
    }

    #[test]
    fn init_parses_defaults_and_optional_flags() {
        let generated_key = ApiKeySecret::generate();
        let key = generated_key.expose_secret();
        let parsed = parse_cli_args_with_env(
            &args(&[
                "aether-mcp",
                "--init",
                "--vault",
                "/tmp/my-vault",
                "--http-addr",
                "127.0.0.1:4771",
                "--api-key",
                key,
                "--watch-ms",
                "0",
            ]),
            |name| (name == "AETHER_PLUGIN_DIR").then(|| "/opt/aether-plugin".to_string()),
        )
        .expect("--init with optional flags must parse");

        assert_eq!(parsed.mode, RunMode::Init);
        assert_eq!(parsed.vault_root, PathBuf::from("/tmp/my-vault"));
        assert_eq!(
            parsed.http_addr,
            Some("127.0.0.1:4771".parse().expect("valid addr"))
        );
        assert_eq!(
            parsed.api_key.as_ref().map(ApiKeySecret::expose_secret),
            Some(key)
        );
        assert_eq!(parsed.watch_ms_explicit, Some(0));
        assert_eq!(parsed.watch_interval, None);
        assert_eq!(parsed.plugin_dir, Some(PathBuf::from("/opt/aether-plugin")));
    }

    #[test]
    fn init_parses_without_http_addr() {
        let parsed = parse_cli_args(&args(&["aether-mcp", "--init", "--vault", "/tmp/my-vault"]))
            .expect("--init without --http-addr must succeed");

        assert_eq!(parsed.mode, RunMode::Init);
        assert_eq!(parsed.http_addr, None);
    }

    #[test]
    fn init_vault_copies_plugin_from_aether_plugin_dir_writes_data_json_and_mcp_snippet() {
        let vault = tempfile::tempdir().expect("create temp vault");
        let plugin_src = tempfile::tempdir().expect("create temp plugin dir");
        let cwd = tempfile::tempdir().expect("create temp cwd");
        fs::write(
            plugin_src.path().join("main.js"),
            "console.log('aether');\n",
        )
        .expect("write main.js");
        fs::write(
            plugin_src.path().join("manifest.json"),
            r#"{"id":"aether-memory","version":"0.4.0"}"#,
        )
        .expect("write manifest.json");
        fs::write(
            plugin_src.path().join("styles.css"),
            ".aether-inbox-card { padding: 8px; }\n",
        )
        .expect("write styles.css");

        let generated_key = ApiKeySecret::generate();
        let key = generated_key.expose_secret();
        let vault_str = vault.path().to_str().expect("utf8 path");
        let plugin_str = plugin_src.path().to_str().expect("utf8 path");
        let parsed = parse_cli_args_with_env(
            &args(&[
                "aether-mcp",
                "--init",
                "--vault",
                vault_str,
                "--http-addr",
                "127.0.0.1:4771",
                "--api-key",
                key,
                "--watch-ms",
                "0",
            ]),
            |name| (name == "AETHER_PLUGIN_DIR").then(|| plugin_str.to_string()),
        )
        .expect("parse --init args");

        let exe = PathBuf::from("/usr/local/bin/aether-mcp");
        let report = init_vault_plugin(&parsed, cwd.path(), &exe).expect("init_vault_plugin");

        assert!(report.copied_main_js);
        assert!(report.copied_manifest_json);
        assert!(report.copied_styles_css);
        let dest = vault.path().join(".obsidian/plugins/aether-memory");
        assert_eq!(report.plugin_dir, dest);
        assert_eq!(
            fs::read_to_string(dest.join("main.js")).expect("read main.js"),
            "console.log('aether');\n"
        );
        assert_eq!(
            fs::read_to_string(dest.join("manifest.json")).expect("read manifest.json"),
            r#"{"id":"aether-memory","version":"0.4.0"}"#
        );
        assert_eq!(
            fs::read_to_string(dest.join("styles.css")).expect("read styles.css"),
            ".aether-inbox-card { padding: 8px; }\n"
        );

        let data: Value =
            serde_json::from_str(&fs::read_to_string(&report.data_json_path).expect("read data"))
                .expect("parse data.json");
        assert_eq!(data["endpointUrl"], "http://127.0.0.1:4771");
        assert_eq!(data["apiKey"], key);
        assert_eq!(data["liveSyncEnabled"], true);

        let server = &report.mcp_config["mcpServers"]["aether-vault"];
        assert_eq!(server["command"], "/usr/local/bin/aether-mcp");
        let expected_vault = fs::canonicalize(vault.path())
            .expect("canonical vault")
            .display()
            .to_string();
        assert_eq!(
            server["args"],
            json!(["--vault", expected_vault, "--watch-ms", "0"])
        );
        assert_eq!(server["env"]["AETHER_API_KEY"], key);
    }

    #[test]
    fn init_vault_defaults_endpoint_to_14771_copies_from_cwd_and_handles_missing_files() {
        let vault = tempfile::tempdir().expect("create temp vault");
        let cwd = tempfile::tempdir().expect("create temp cwd");
        let cwd_plugin = cwd.path().join("obsidian-aether-plugin");
        fs::create_dir_all(&cwd_plugin).expect("create cwd plugin dir");
        fs::write(
            cwd_plugin.join("manifest.json"),
            r#"{"id":"aether-memory"}"#,
        )
        .expect("write manifest.json");

        let vault_str = vault.path().to_str().expect("utf8 path");
        let parsed = parse_cli_args_with_env(
            &args(&["aether-mcp", "--init", "--vault", vault_str]),
            |_| None,
        )
        .expect("parse --init args");
        let report = init_vault_plugin(&parsed, cwd.path(), Path::new("aether-mcp"))
            .expect("init without main.js must succeed");

        assert!(!report.copied_main_js);
        assert!(report.copied_manifest_json);
        assert!(!report.copied_styles_css);
        assert_eq!(
            report.plugin_dir,
            vault.path().join(".obsidian/plugins/aether-memory")
        );
        assert_eq!(report.endpoint_url, "http://127.0.0.1:14771");

        let data: Value =
            serde_json::from_str(&fs::read_to_string(&report.data_json_path).expect("read data"))
                .expect("parse data.json");
        assert_eq!(data["endpointUrl"], "http://127.0.0.1:14771");
        assert_eq!(data["apiKey"], "");
        assert!(report.mcp_config["mcpServers"]["aether-vault"]
            .get("env")
            .is_none());
    }

    #[test]
    fn init_vault_preserves_existing_settings_and_maps_unspecified_addr() {
        let vault = tempfile::tempdir().expect("create temp vault");
        let cwd = tempfile::tempdir().expect("create temp cwd");
        let vault_str = vault.path().to_str().expect("utf8 path");

        let first_parsed = parse_cli_args_with_env(
            &args(&["aether-mcp", "--init", "--vault", vault_str]),
            |_| None,
        )
        .expect("parse first --init");
        let first = init_vault_plugin(&first_parsed, cwd.path(), Path::new("aether-mcp"))
            .expect("first init");

        let mut updated: Map<String, Value> = serde_json::from_str(
            &fs::read_to_string(&first.data_json_path).expect("read initial data.json"),
        )
        .expect("parse initial data.json");
        updated.insert("syncDebounceMs".to_string(), json!(3000));
        updated.insert("apiKey".to_string(), json!("aek_existing"));
        fs::write(
            &first.data_json_path,
            serde_json::to_string_pretty(&Value::Object(updated)).expect("serialize"),
        )
        .expect("write updated data.json");

        let rerun_parsed = parse_cli_args_with_env(
            &args(&[
                "aether-mcp",
                "--init",
                "--vault",
                vault_str,
                "--http-addr",
                "0.0.0.0:14771",
            ]),
            |_| None,
        )
        .expect("parse rerun");
        let rerun = init_vault_plugin(&rerun_parsed, cwd.path(), Path::new("aether-mcp"))
            .expect("rerun init");
        assert_eq!(rerun.endpoint_url, "http://127.0.0.1:14771");
        let rerun_data: Value =
            serde_json::from_str(&fs::read_to_string(&rerun.data_json_path).expect("read rerun"))
                .expect("parse rerun data.json");
        assert_eq!(rerun_data["endpointUrl"], "http://127.0.0.1:14771");
        assert_eq!(rerun_data["syncDebounceMs"], 3000);
        assert_eq!(rerun_data["apiKey"], "aek_existing");
    }

    #[test]
    fn init_vault_rejects_non_directory_vault_path() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let file_vault = dir.path().join("not-a-dir.md");
        fs::write(&file_vault, "hello").expect("write file");
        let parsed = parse_cli_args(&args(&[
            "aether-mcp",
            "--init",
            "--vault",
            file_vault.to_str().expect("utf8 path"),
        ]))
        .expect("parse args");

        let err = init_vault_plugin(&parsed, dir.path(), Path::new("aether-mcp"))
            .expect_err("file vault path must fail");
        assert!(matches!(err, AetherError::InvalidInput(_)));
    }

    #[test]
    fn init_vault_forwards_optional_db_and_kv_flags_and_rejects_corrupt_data_json() {
        let vault = tempfile::tempdir().expect("create temp vault");
        let cwd = tempfile::tempdir().expect("create temp cwd");
        let vault_str = vault.path().to_str().expect("utf8 path");

        let parsed = parse_cli_args_with_env(
            &args(&[
                "aether-mcp",
                "--init",
                "--vault",
                vault_str,
                "--db",
                "custom.sqlite",
                "--kv-endpoint",
                "http://127.0.0.1:8000",
                "--kv-model",
                "llama-3.1-8b",
            ]),
            |_| None,
        )
        .expect("parse optional flags");
        let report = init_vault_plugin(&parsed, cwd.path(), Path::new("aether-mcp"))
            .expect("init with optional flags");

        let data: Value =
            serde_json::from_str(&fs::read_to_string(&report.data_json_path).expect("read data"))
                .expect("parse data.json");
        assert_eq!(data["kvEndpoint"], "http://127.0.0.1:8000");
        assert_eq!(data["kvModel"], "llama-3.1-8b");

        let expected_vault = fs::canonicalize(vault.path())
            .expect("canonical vault")
            .display()
            .to_string();
        let expected_db = cwd.path().join("custom.sqlite").display().to_string();
        assert_eq!(
            report.mcp_config["mcpServers"]["aether-vault"]["args"],
            json!([
                "--vault",
                expected_vault,
                "--db",
                expected_db,
                "--kv-endpoint",
                "http://127.0.0.1:8000",
                "--kv-model",
                "llama-3.1-8b"
            ])
        );

        fs::write(&report.data_json_path, "{not-valid-json").expect("corrupt data.json");
        let err = init_vault_plugin(&parsed, cwd.path(), Path::new("aether-mcp"))
            .expect_err("corrupt existing data.json must fail");
        assert!(matches!(err, AetherError::Serialization(_)));
    }

    #[tokio::test]
    async fn run_http_only_serves_requests_until_shutdown_signal() {
        let vault = tempfile::tempdir().expect("create temp vault");
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

    #[test]
    fn parse_cli_args_parses_service_subcommands_and_rejects_invalid() {
        let install = parse_cli_args_with_env(
            &[
                "aether-mcp".into(),
                "service".into(),
                "install".into(),
                "--vault".into(),
                "/tmp/my-vault".into(),
            ],
            |_| None,
        )
        .expect("parse service install");
        assert_eq!(install.mode, RunMode::Service(ServiceAction::Install));
        assert_eq!(install.vault_root, PathBuf::from("/tmp/my-vault"));
        assert_eq!(
            install.http_addr,
            Some("127.0.0.1:14771".parse().expect("default addr"))
        );

        let uninstall = parse_cli_args_with_env(
            &["aether-mcp".into(), "service".into(), "uninstall".into()],
            |_| None,
        )
        .expect("parse service uninstall");
        assert_eq!(uninstall.mode, RunMode::Service(ServiceAction::Uninstall));

        let status = parse_cli_args_with_env(
            &["aether-mcp".into(), "service".into(), "status".into()],
            |_| None,
        )
        .expect("parse service status");
        assert_eq!(status.mode, RunMode::Service(ServiceAction::Status));

        let missing = parse_cli_args_with_env(&["aether-mcp".into(), "service".into()], |_| None)
            .err()
            .expect("missing service action must fail");
        assert!(matches!(missing, AetherError::InvalidInput(_)));
    }

    #[test]
    fn render_service_definitions_include_vault_addr_and_escape_xml() {
        let spec = ServiceSpec {
            exe: PathBuf::from("/usr/local/bin/aether-mcp"),
            vault_root: PathBuf::from("/home/alice/My Vault & Notes"),
            http_addr: "127.0.0.1:14771".parse().expect("valid addr"),
            watch_ms: 2000,
        };

        let unit = render_systemd_unit(&spec);
        assert!(unit.contains("ExecStart=\"/usr/local/bin/aether-mcp\" --vault \"/home/alice/My Vault & Notes\" --watch-ms 2000 --http-addr 127.0.0.1:14771 --http-only"));
        assert!(unit.contains("Restart=on-failure"));

        let plist = render_launchd_plist(&spec, Path::new("/home/alice/Library/Logs/aether-mcp"));
        assert!(plist.contains("<string>hq.muninn.aether-mcp</string>"));
        assert!(plist.contains("<string>/home/alice/My Vault &amp; Notes</string>"));
        assert!(plist.contains("<true/>"));

        let win_args = windows_task_args(&spec);
        assert!(win_args.contains(&"AetherMcpService".to_string()));
        assert!(win_args.contains(&"ONLOGON".to_string()));
    }

    #[test]
    fn default_vault_uses_home_aether_vault_when_unset_and_respects_override() {
        let with_home = parse_cli_args_with_env(&args(&["aether-mcp"]), |name| {
            (name == "HOME").then(|| "/home/alice".to_string())
        })
        .expect("default args with HOME must succeed");
        assert_eq!(
            with_home.vault_root,
            PathBuf::from("/home/alice").join(".aether").join("vault")
        );

        let with_override = parse_cli_args_with_env(
            &args(&["aether-mcp", "--vault", "/tmp/custom-vault"]),
            |name| (name == "HOME").then(|| "/home/alice".to_string()),
        )
        .expect("explicit --vault must override HOME default");
        assert_eq!(with_override.vault_root, PathBuf::from("/tmp/custom-vault"));
    }

    #[test]
    fn parse_cli_args_parses_doctor_and_install_clients_flags() {
        let doctor = parse_cli_args(&args(&["aether-mcp", "doctor", "--vault", "/tmp/v"]))
            .expect("doctor subcommand must parse");
        assert_eq!(doctor.mode, RunMode::Doctor);
        assert_eq!(doctor.vault_root, PathBuf::from("/tmp/v"));

        let init_clients = parse_cli_args(&args(&[
            "aether-mcp",
            "--init",
            "--install-clients",
            "--vault",
            "/tmp/v",
        ]))
        .expect("--init --install-clients must parse");
        assert_eq!(init_clients.mode, RunMode::Init);
        assert!(init_clients.install_clients);

        let bad_combo = parse_cli_args(&args(&[
            "aether-mcp",
            "doctor",
            "--init",
            "--vault",
            "/tmp/v",
        ]))
        .err()
        .expect("doctor + --init must fail");
        assert!(matches!(bad_combo, AetherError::InvalidInput(_)));
    }

    #[test]
    fn merge_mcp_server_into_config_creates_preserves_and_rejects_corrupt() {
        let entry = json!({
            "command": "/usr/local/bin/aether-mcp",
            "args": ["--vault", "/tmp/vault", "--watch-ms", "0"]
        });

        let fresh = merge_mcp_server_into_config(None, "aether-vault", &entry)
            .expect("fresh config merge must succeed");
        let fresh_val: Value = serde_json::from_str(&fresh).expect("valid JSON");
        assert_eq!(fresh_val["mcpServers"]["aether-vault"], entry);

        let existing = r#"{
  "theme": "dark",
  "mcpServers": {
    "other-server": { "command": "npx", "args": ["-y", "other"] }
  }
}"#;
        let merged = merge_mcp_server_into_config(Some(existing), "aether-vault", &entry)
            .expect("existing config merge must succeed");
        let merged_val: Value = serde_json::from_str(&merged).expect("valid JSON");
        assert_eq!(merged_val["theme"], json!("dark"));
        assert_eq!(
            merged_val["mcpServers"]["other-server"]["command"],
            json!("npx")
        );
        assert_eq!(merged_val["mcpServers"]["aether-vault"], entry);

        let corrupt = merge_mcp_server_into_config(Some("{not valid"), "aether-vault", &entry)
            .expect_err("corrupt JSON must fail");
        assert!(!corrupt.to_string().is_empty());

        let bad_servers =
            merge_mcp_server_into_config(Some(r#"{"mcpServers": []}"#), "aether-vault", &entry)
                .expect_err("non-object mcpServers must fail");
        assert!(matches!(bad_servers, AetherError::InvalidInput(_)));
    }

    #[test]
    fn install_mcp_clients_updates_detected_client_configs_idempotently() {
        let home = tempfile::tempdir().expect("tempdir");
        let cursor_dir = home.path().join(".cursor");
        let claude_dir = home.path().join(".claude");
        let antigravity_dir = home.path().join(".gemini").join("antigravity");
        fs::create_dir_all(&cursor_dir).expect("create .cursor");
        fs::create_dir_all(&claude_dir).expect("create .claude");
        fs::create_dir_all(&antigravity_dir).expect("create antigravity");

        fs::write(
            cursor_dir.join("mcp.json"),
            r#"{"mcpServers":{"existing":{"command":"echo"}}}"#,
        )
        .expect("seed cursor mcp.json");

        let entry = json!({
            "command": "aether-mcp",
            "args": ["--vault", "/tmp/v", "--watch-ms", "0"]
        });

        let all = ["Cursor", "Claude Code", "Antigravity"];
        let updated = install_mcp_clients(home.path(), &entry, &all)
            .expect("install_mcp_clients must succeed");
        assert_eq!(updated.len(), 3);
        assert!(updated.contains(&cursor_dir.join("mcp.json")));
        assert!(updated.contains(&home.path().join(".claude.json")));
        assert!(updated.contains(&antigravity_dir.join("mcp_config.json")));

        let cursor_json: Value = serde_json::from_str(
            &fs::read_to_string(cursor_dir.join("mcp.json")).expect("read cursor"),
        )
        .expect("parse cursor");
        assert_eq!(
            cursor_json["mcpServers"]["existing"]["command"],
            json!("echo")
        );
        assert_eq!(cursor_json["mcpServers"]["aether-vault"], entry);

        // Idempotent second run
        let second = install_mcp_clients(home.path(), &entry, &all)
            .expect("second install_mcp_clients must succeed");
        assert_eq!(second.len(), 3);
    }

    #[test]
    fn install_mcp_clients_writes_only_the_selected_clients() {
        let home = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(home.path().join(".cursor")).expect("create .cursor");
        fs::create_dir_all(home.path().join(".claude")).expect("create .claude");
        let entry = json!({"command": "aether-mcp"});

        let updated =
            install_mcp_clients(home.path(), &entry, &["Cursor"]).expect("install must succeed");
        assert_eq!(updated, vec![home.path().join(".cursor").join("mcp.json")]);
        assert!(!home.path().join(".claude.json").exists());

        let none = install_mcp_clients(home.path(), &entry, &[]).expect("install must succeed");
        assert!(none.is_empty());
    }

    #[test]
    fn detected_client_names_lists_only_clients_with_a_config_or_folder() {
        let home = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(home.path().join(".cursor")).expect("create .cursor");
        assert_eq!(detected_client_names(home.path()), vec!["Cursor"]);
    }

    #[test]
    fn uninstall_parses_alone_and_rejects_init_and_http_only() {
        let parsed = parse_cli_args(&args(&["aether-mcp", "uninstall"]))
            .expect("uninstall subcommand must parse");
        assert_eq!(parsed.mode, RunMode::Uninstall);
        for extra in [
            &["--init"][..],
            &["--http-only", "--http-addr", "127.0.0.1:1"][..],
        ] {
            let mut argv = vec!["aether-mcp", "uninstall"];
            argv.extend_from_slice(extra);
            assert!(parse_cli_args(&args(&argv)).is_err(), "{extra:?} must fail");
        }
    }

    #[test]
    fn uninstall_flags_parse_and_purge_needs_an_explicit_vault() {
        let parsed = parse_cli_args(&args(&[
            "aether-mcp",
            "uninstall",
            "--yes",
            "--purge-vault",
            "--vault",
            "/v",
        ]))
        .expect("uninstall flags must parse");
        assert!(parsed.assume_yes && parsed.purge_vault && parsed.vault_explicit);
        assert!(
            !parse_cli_args(&args(&["aether-mcp", "uninstall"]))
                .expect("plain uninstall parses")
                .purge_vault
        );
        assert!(parse_cli_args_with_env(
            &args(&["aether-mcp", "uninstall", "--purge-vault"]),
            |_| None
        )
        .is_err());
    }

    #[test]
    fn run_doctor_checks_vault_and_client_configs() {
        let home = tempfile::tempdir().expect("temp home");
        let vault = tempfile::tempdir().expect("temp vault");
        fs::create_dir_all(vault.path().join(".aether")).expect("create .aether");
        fs::write(vault.path().join(".aether").join("aether.sqlite"), b"").expect("seed sqlite");

        let report = collect_doctor_report(vault.path(), Some(home.path()))
            .expect("doctor on valid vault must succeed");
        assert!(report.vault_exists);
        assert!(report.sqlite_exists);

        let missing = collect_doctor_report(&vault.path().join("nonexistent"), Some(home.path()))
            .expect("doctor report on missing vault should still collect status");
        assert!(!missing.vault_exists);
    }

    #[test]
    fn help_flag_returns_usage_text() {
        let err = parse_cli_args(&args(&["aether-mcp", "--help"]))
            .err()
            .expect("--help must not start the server");
        assert!(err.to_string().contains("Usage"), "got: {err}");
    }

    #[test]
    fn unknown_flag_returns_invalid_input() {
        let err = parse_cli_args(&args(&["aether-mcp", "--bogus"]))
            .err()
            .expect("unknown flag must fail");
        assert!(matches!(err, AetherError::InvalidInput(_)), "got {err:?}");
    }

    #[test]
    fn non_numeric_watch_ms_returns_invalid_input() {
        let err = parse_cli_args(&args(&["aether-mcp", "--watch-ms", "abc"]))
            .err()
            .expect("non-numeric --watch-ms must fail");
        assert!(matches!(err, AetherError::InvalidInput(_)), "got {err:?}");
    }

    #[test]
    fn flag_overrides_env_for_http_addr() {
        let parsed = parse_cli_args_with_env(
            &args(&["aether-mcp", "--http-addr", "127.0.0.1:1"]),
            |name| (name == "AETHER_HTTP_ADDR").then(|| "127.0.0.1:2".to_string()),
        )
        .expect("flag and env together must parse");
        assert_eq!(parsed.http_addr, Some("127.0.0.1:1".parse().expect("addr")));
    }

    #[test]
    fn options_work_after_a_subcommand_and_doctor_rejects_init() {
        let parsed = parse_cli_args(&args(&["aether-mcp", "doctor", "--vault", "/tmp/v"]))
            .expect("doctor with --vault must parse");
        assert_eq!(parsed.mode, RunMode::Doctor);
        let err = parse_cli_args(&args(&["aether-mcp", "service", "install", "--http-only"]))
            .err()
            .expect("service + --http-only must fail");
        assert!(matches!(err, AetherError::InvalidInput(_)), "got {err:?}");
    }

    #[test]
    fn completions_script_mentions_service_subcommand() {
        let script = cli::completions_script(clap_complete::Shell::Bash);
        assert!(script.contains("service"), "got: {script}");
    }

    #[test]
    fn doctor_plain_output_matches_the_previous_text() {
        let report = DoctorReport {
            vault_root: PathBuf::from("/v"),
            vault_exists: true,
            sqlite_exists: true,
            configured_clients: vec![("Cursor", PathBuf::from("/c/mcp.json"), true)],
        };
        let text = ui::render_plain(&doctor_lines(&report, Path::new("/bin/aether-mcp")));
        assert_eq!(
            text,
            "aether-mcp binary: /bin/aether-mcp\nvault path: /v (ok)\nsqlite index: ok (.aether/aether.sqlite)\nmcp clients:\n  Cursor: configured (/c/mcp.json)\n"
        );
    }

    #[test]
    fn doctor_lines_add_fix_hints_for_problems() {
        let report = DoctorReport {
            vault_root: PathBuf::from("/v"),
            vault_exists: false,
            sqlite_exists: false,
            configured_clients: vec![("Cursor", PathBuf::from("/c/mcp.json"), false)],
        };
        let lines = doctor_lines(&report, Path::new("/bin/aether-mcp"));
        let vault = &lines[1];
        assert_eq!(vault.level, ui::Level::Fail);
        assert!(vault.hint.as_deref().is_some_and(|h| h.contains("--vault")));
        let client = lines.last().expect("client line");
        assert_eq!(client.level, ui::Level::Warn);
        assert!(client
            .hint
            .as_deref()
            .is_some_and(|h| h.contains("--init --install-clients")));
    }
}
