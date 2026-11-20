use std::fs;
use std::future::Future;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use nytheris_aether_core::{AetherError, Result};
use nytheris_aether_vault::{
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
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let parsed = parse_cli_args(&args)?;
    match parsed.mode {
        RunMode::Init => run_init(&parsed),
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
    let report = init_vault_plugin(parsed, &cwd, &exe)?;
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
    eprintln!("\nPaste this into mcp.json (Cursor / Claude Desktop / Antigravity):");
    let rendered = serde_json::to_string_pretty(&report.mcp_config)?;
    println!("{rendered}");
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
    let mut vault = env("AETHER_VAULT_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let mut db = None;
    let mut watch_ms: u64 = 2000;
    let mut watch_ms_explicit: Option<u64> = None;
    let mut http_addr_raw = env("AETHER_HTTP_ADDR");
    let mut http_only = false;
    let mut init = false;
    let mut require_auth = env("AETHER_REQUIRE_AUTH")
        .is_some_and(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes"));
    let mut bootstrap_raw = env("AETHER_BOOTSTRAP_ADMIN_KEY");
    let mut api_key_raw = env("AETHER_API_KEY");
    let mut kv_endpoint = env("AETHER_KV_ENDPOINT");
    let mut kv_model_raw = env("AETHER_KV_MODEL");
    let plugin_dir = env("AETHER_PLUGIN_DIR").map(PathBuf::from);

    let mut service_action: Option<ServiceAction> = None;
    let mut idx = 1usize;
    if args.get(1).map(String::as_str) == Some("service") {
        service_action = Some(match args.get(2).map(String::as_str) {
            Some("install") => ServiceAction::Install,
            Some("uninstall") => ServiceAction::Uninstall,
            Some("status") => ServiceAction::Status,
            _ => {
                return Err(AetherError::InvalidInput(
                    "usage: aether-mcp service <install|uninstall|status> [--vault <dir>] [--http-addr <addr>] [--watch-ms <ms>]"
                        .into(),
                ));
            }
        });
        idx = 3;
    }

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
                if let Ok(parsed_ms) = args[idx + 1].parse::<u64>() {
                    watch_ms = parsed_ms;
                    watch_ms_explicit = Some(parsed_ms);
                } else {
                    watch_ms = 2000;
                }
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
            "--init" => {
                init = true;
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
        None if service_action == Some(ServiceAction::Install) => Some(
            "127.0.0.1:14771"
                .parse::<SocketAddr>()
                .expect("default loopback addr"),
        ),
        None => None,
    };
    let mode = match (service_action, init, http_only) {
        (Some(_), true, _) | (Some(_), _, true) => {
            return Err(AetherError::InvalidInput(
                "`service` cannot be combined with `--init` or `--http-only`".into(),
            ));
        }
        (Some(action), false, false) => RunMode::Service(action),
        (None, true, true) => {
            return Err(AetherError::InvalidInput(
                "`--init` and `--http-only` cannot be used together".into(),
            ));
        }
        (None, true, false) => RunMode::Init,
        (None, false, true) => {
            if http_addr.is_none() {
                return Err(AetherError::InvalidInput(
                    "`--http-only` requires `--http-addr <addr>`".into(),
                ));
            }
            RunMode::HttpOnly
        }
        (None, false, false) => RunMode::StdioMcp,
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
  <string>ai.nytheris.aether-mcp</string>\n\
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
                println!(
                    "Installed and started systemd user service: {}",
                    unit_path.display()
                );
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
                println!("Uninstalled aether-mcp systemd user service.");
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
        let plist_path = home.join("Library/LaunchAgents/ai.nytheris.aether-mcp.plist");
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
                println!(
                    "Installed and loaded launchd agent: {}",
                    plist_path.display()
                );
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
                println!("Uninstalled aether-mcp launchd agent.");
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
                    .args(["list", "ai.nytheris.aether-mcp"])
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
                println!("Installed and started Windows scheduled task: AetherMcpService");
                Ok(())
            }
            ServiceAction::Uninstall => {
                let _ = std::process::Command::new("schtasks")
                    .args(["/End", "/TN", "AetherMcpService"])
                    .output();
                let _ = std::process::Command::new("schtasks")
                    .args(["/Delete", "/F", "/TN", "AetherMcpService"])
                    .output();
                println!("Uninstalled Windows scheduled task: AetherMcpService");
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
        assert!(plist.contains("<string>ai.nytheris.aether-mcp</string>"));
        assert!(plist.contains("<string>/home/alice/My Vault &amp; Notes</string>"));
        assert!(plist.contains("<true/>"));

        let win_args = windows_task_args(&spec);
        assert!(win_args.contains(&"AetherMcpService".to_string()));
        assert!(win_args.contains(&"ONLOGON".to_string()));
    }
}
