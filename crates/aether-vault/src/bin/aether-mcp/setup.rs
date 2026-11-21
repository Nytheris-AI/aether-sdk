//! `aether-mcp setup`: a guided first-run flow.
//!
//! It finds Obsidian vaults, installs the background service, configures detected MCP clients,
//! verifies the result, and prints what is left to do. Questions are asked only in an interactive
//! terminal. `--yes` accepts the defaults, and without a terminal it refuses to guess.

use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use muninn_aether_core::{AetherError, Result};
use serde_json::Value;

use super::ui::{Level, Line, Ui};
use super::{
    build_mcp_json_snippet, collect_doctor_report, doctor_lines, install_mcp_clients,
    known_mcp_client_configs, run_service, CliArgs, ServiceAction,
};

const PLUGIN_REPO_URL: &str = "https://github.com/muninn-hq/aether-memory";
const QUICKSTART_URL: &str = "https://muninn-hq.github.io/aether-sdk/docs/get-started/quickstart";

/// Vault paths listed in Obsidian's `obsidian.json`, most recently used first.
pub fn parse_obsidian_vaults(raw: &str) -> Vec<PathBuf> {
    let Ok(Value::Object(root)) = serde_json::from_str::<Value>(raw) else {
        return Vec::new();
    };
    let Some(Value::Object(vaults)) = root.get("vaults") else {
        return Vec::new();
    };
    let mut found: Vec<(u64, PathBuf)> = vaults
        .values()
        .filter_map(|v| {
            let path = v.get("path")?.as_str()?;
            Some((
                v.get("ts").and_then(Value::as_u64).unwrap_or(0),
                PathBuf::from(path),
            ))
        })
        .collect();
    found.sort_by_key(|(ts, _)| std::cmp::Reverse(*ts));
    found.into_iter().map(|(_, path)| path).collect()
}

fn obsidian_config_paths(home: &Path, appdata: Option<&Path>) -> Vec<PathBuf> {
    let mut paths = vec![
        home.join(".config/obsidian/obsidian.json"),
        home.join(".var/app/md.obsidian.Obsidian/config/obsidian/obsidian.json"),
        home.join("Library/Application Support/obsidian/obsidian.json"),
    ];
    if let Some(appdata) = appdata {
        paths.push(appdata.join("obsidian/obsidian.json"));
    }
    paths
}

/// Existing vault directories known to Obsidian, without duplicates.
pub fn discover_vaults(home: &Path, appdata: Option<&Path>) -> Vec<PathBuf> {
    let mut vaults: Vec<PathBuf> = Vec::new();
    for config in obsidian_config_paths(home, appdata) {
        let Ok(raw) = std::fs::read_to_string(config) else {
            continue;
        };
        for path in parse_obsidian_vaults(&raw) {
            if path.is_dir() && !vaults.contains(&path) {
                vaults.push(path);
            }
        }
    }
    vaults
}

#[derive(Debug, PartialEq, Eq)]
pub enum VaultChoice {
    /// The user named a vault, or exactly one was found.
    Use(PathBuf),
    /// Several were found, so ask which one.
    Pick(Vec<PathBuf>),
    /// None were found, so ask for a path.
    Ask,
}

pub fn choose_vault(explicit: Option<PathBuf>, found: Vec<PathBuf>) -> VaultChoice {
    match (explicit, found.len()) {
        (Some(path), _) => VaultChoice::Use(path),
        (None, 0) => VaultChoice::Ask,
        (None, 1) => VaultChoice::Use(found.into_iter().next().expect("one vault")),
        (None, _) => VaultChoice::Pick(found),
    }
}

/// `--watch-ms` for the MCP client entry: `0` when the background service watches the vault.
fn stdio_watch_ms(service_installed: bool) -> Option<u64> {
    service_installed.then_some(0)
}

fn wait_for_server(addr: SocketAddr, attempts: u32) -> bool {
    for _ in 0..attempts {
        if TcpStream::connect_timeout(&addr, Duration::from_millis(500)).is_ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    false
}

/// Performs the setup steps for `parsed.vault_root` and reports each result.
/// Nothing here asks questions, so it can be tested and reused by `--yes`.
pub fn apply_setup(
    parsed: &mut CliArgs,
    home: &Path,
    cwd: &Path,
    exe: &Path,
    install_service: bool,
    install_clients: bool,
) -> Vec<Line> {
    let mut lines = Vec::new();
    let vault = parsed.vault_root.clone();
    let vault = &vault;
    lines.push(Line::new(Level::Ok, format!("Vault: {}", vault.display())));

    if install_service {
        match run_service(ServiceAction::Install, parsed) {
            Ok(()) => {}
            Err(err) => lines.push(
                Line::new(Level::Fail, format!("Background service: {err}")).hint(format!(
                    "run the server in the foreground instead: aether-mcp --vault {} --http-addr 127.0.0.1:14771 --http-only",
                    vault.display()
                )),
            ),
        }
        let addr = parsed
            .http_addr
            .unwrap_or_else(|| "127.0.0.1:14771".parse().expect("default addr"));
        lines.push(if wait_for_server(addr, 6) {
            Line::new(Level::Ok, format!("Server is answering on {addr}"))
        } else {
            Line::new(
                Level::Warn,
                format!("Server is not answering on {addr} yet"),
            )
            .hint("run `aether-mcp service status` to see why")
        });
    } else {
        lines.push(Line::new(Level::Info, "Background service: skipped"));
    }

    // The service already scans the vault, so the stdio session must not scan it again.
    if let Some(ms) = stdio_watch_ms(install_service) {
        parsed.watch_ms_explicit = Some(ms);
    }
    let report = collect_doctor_report(vault, Some(home));
    let pending = report
        .as_ref()
        .map(|r| r.configured_clients.iter().filter(|c| !c.2).count())
        .unwrap_or(0);
    if install_clients && pending > 0 {
        let entry = build_mcp_json_snippet(parsed, vault, cwd, exe)
            .get("mcpServers")
            .and_then(|m| m.get("aether-vault"))
            .cloned();
        match entry.map(|e| install_mcp_clients(home, &e)) {
            Some(Ok(paths)) => {
                for path in paths {
                    lines.push(Line::new(
                        Level::Ok,
                        format!("MCP client configured: {}", path.display()),
                    ));
                }
            }
            Some(Err(err)) => lines.push(Line::new(Level::Fail, format!("MCP clients: {err}"))),
            None => {}
        }
    } else if known_mcp_client_configs(home)
        .iter()
        .any(|(_, dir, file)| dir.is_dir() || file.is_file())
        && pending == 0
    {
        lines.push(Line::new(Level::Ok, "MCP clients already configured"));
    }

    if let Ok(report) = collect_doctor_report(vault, Some(home)) {
        lines.extend(doctor_lines(&report, exe));
    }
    lines
}

fn next_steps() -> String {
    format!(
        "1. In Obsidian, install the Obsidian42 BRAT plugin, choose Add beta plugin, and enter {PLUGIN_REPO_URL}\n2. Enable AMC under Settings, Community plugins\n3. Ask your agent a question your notes answer\n\nGuide: {QUICKSTART_URL}"
    )
}

fn prompt_err(err: std::io::Error) -> AetherError {
    AetherError::InvalidInput(format!("setup was cancelled: {err}"))
}

fn ask_vault(choice: VaultChoice, assume_yes: bool, found_explicitly: bool) -> Result<PathBuf> {
    match choice {
        VaultChoice::Use(path) => {
            if !assume_yes
                && !found_explicitly
                && !cliclack::confirm(format!("Use the vault at {}?", path.display()))
                    .initial_value(true)
                    .interact()
                    .map_err(prompt_err)?
            {
                return ask_path();
            }
            Ok(path)
        }
        VaultChoice::Pick(vaults) => {
            if assume_yes {
                return Ok(vaults.into_iter().next().expect("at least two vaults"));
            }
            let mut select = cliclack::select("Which vault should agents use?");
            for (idx, path) in vaults.iter().enumerate() {
                select = select.item(idx, path.display().to_string(), "");
            }
            let idx = select.interact().map_err(prompt_err)?;
            Ok(vaults.into_iter().nth(idx).expect("selected index"))
        }
        VaultChoice::Ask => {
            if assume_yes {
                return Err(AetherError::InvalidInput(
                    "no Obsidian vault found, pass --vault <path>".into(),
                ));
            }
            ask_path()
        }
    }
}

fn ask_path() -> Result<PathBuf> {
    let path: String = cliclack::input("Path to your Obsidian vault")
        .validate(|value: &String| {
            if Path::new(value.trim()).is_dir() {
                Ok(())
            } else {
                Err("that folder does not exist")
            }
        })
        .interact()
        .map_err(prompt_err)?;
    Ok(PathBuf::from(path.trim()))
}

pub fn run_setup(mut parsed: CliArgs) -> Result<()> {
    let ui = Ui::detect();
    let home = super::resolve_home_dir()?;
    let cwd = std::env::current_dir().map_err(|err| AetherError::Permanent {
        connector: "aether-setup",
        message: format!("failed to resolve current directory: {err}"),
    })?;
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("aether-mcp"));
    let appdata = std::env::var_os("APPDATA").map(PathBuf::from);

    if !ui.fancy() && !parsed.assume_yes {
        return Err(AetherError::InvalidInput(
            "setup asks questions, so it needs an interactive terminal. Run it in a terminal, or pass --yes to accept the defaults (and --vault <path> to name the vault)."
                .into(),
        ));
    }

    ui.intro("aether-mcp setup");
    let explicit = parsed.vault_explicit.then(|| parsed.vault_root.clone());
    let was_explicit = explicit.is_some();
    let choice = choose_vault(explicit, discover_vaults(&home, appdata.as_deref()));
    let vault = ask_vault(choice, parsed.assume_yes, was_explicit)?;
    if !vault.is_dir() {
        return Err(AetherError::InvalidInput(format!(
            "vault `{}` is not a folder",
            vault.display()
        )));
    }
    parsed.vault_root = vault;

    let clients = !parsed.skip_clients && {
        let detected: Vec<_> = known_mcp_client_configs(&home)
            .into_iter()
            .filter(|(_, dir, file)| dir.is_dir() || file.is_file())
            .map(|(name, _, _)| name)
            .collect();
        parsed.assume_yes
            || detected.is_empty()
            || cliclack::confirm(format!(
                "Add aether-vault to your MCP clients ({})?",
                detected.join(", ")
            ))
            .initial_value(true)
            .interact()
            .map_err(prompt_err)?
    };

    let install_service = !parsed.skip_service;
    let lines = apply_setup(&mut parsed, &home, &cwd, &exe, install_service, clients);
    ui.print(&lines);
    ui.note("Next steps", &next_steps());
    ui.outro("Setup finished");
    if lines.iter().any(|l| l.level == Level::Fail) {
        return Err(AetherError::InvalidInput(
            "setup finished with problems, see the lines marked above".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_obsidian_vaults_newest_first_and_ignores_bad_input() {
        let raw =
            r#"{"vaults":{"a":{"path":"/old","ts":1},"b":{"path":"/new","ts":9},"c":{"ts":5}}}"#;
        assert_eq!(
            parse_obsidian_vaults(raw),
            vec![PathBuf::from("/new"), PathBuf::from("/old")]
        );
        assert!(parse_obsidian_vaults("not json").is_empty());
        assert!(parse_obsidian_vaults("{}").is_empty());
    }

    #[test]
    fn chooses_vault_from_flag_or_discovery() {
        let a = PathBuf::from("/a");
        let b = PathBuf::from("/b");
        assert_eq!(
            choose_vault(Some(a.clone()), vec![b.clone()]),
            VaultChoice::Use(a.clone())
        );
        assert_eq!(choose_vault(None, vec![]), VaultChoice::Ask);
        assert_eq!(
            choose_vault(None, vec![a.clone()]),
            VaultChoice::Use(a.clone())
        );
        assert_eq!(
            choose_vault(None, vec![a.clone(), b.clone()]),
            VaultChoice::Pick(vec![a, b])
        );
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("aether_setup_{name}_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn setup_args(vault: &Path, extra: &[&str]) -> CliArgs {
        let mut args = vec![
            "aether-mcp".to_string(),
            "setup".to_string(),
            "--vault".to_string(),
            vault.display().to_string(),
        ];
        args.extend(extra.iter().map(|s| (*s).to_string()));
        super::super::parse_cli_args(&args).expect("setup args parse")
    }

    #[test]
    fn setup_flags_parse_and_conflict_with_init() {
        let parsed = setup_args(Path::new("/v"), &["--yes", "--no-clients", "--no-service"]);
        assert_eq!(parsed.mode, super::super::RunMode::Setup);
        assert!(parsed.vault_explicit && parsed.assume_yes);
        assert!(parsed.skip_clients && parsed.skip_service);
        let err = super::super::parse_cli_args(&[
            "aether-mcp".to_string(),
            "setup".to_string(),
            "--init".to_string(),
        ])
        .err()
        .expect("setup + --init must fail");
        assert!(matches!(err, AetherError::InvalidInput(_)));
    }

    #[test]
    fn stdio_session_skips_scanning_only_when_the_service_does_it() {
        assert_eq!(stdio_watch_ms(true), Some(0));
        assert_eq!(stdio_watch_ms(false), None);
    }

    #[test]
    fn apply_setup_configures_clients_once_and_can_skip_them() {
        let home = temp_dir("home");
        let vault = temp_dir("vault");
        std::fs::create_dir_all(home.join(".cursor")).expect("cursor dir");
        let mut parsed = setup_args(&vault, &["--yes", "--no-service"]);
        let exe = Path::new("/bin/aether-mcp");

        let skipped = apply_setup(&mut parsed, &home, &vault, exe, false, false);
        assert!(!home.join(".cursor/mcp.json").exists());
        assert!(skipped
            .iter()
            .any(|l| l.text.contains("Background service: skipped")));

        let first = apply_setup(&mut parsed, &home, &vault, exe, false, true);
        let config = std::fs::read_to_string(home.join(".cursor/mcp.json")).expect("cursor config");
        assert!(config.contains("aether-vault"));
        assert!(first
            .iter()
            .any(|l| l.text.starts_with("MCP client configured")));

        let second = apply_setup(&mut parsed, &home, &vault, exe, false, true);
        assert!(second
            .iter()
            .any(|l| l.text == "MCP clients already configured"));
        assert!(!second
            .iter()
            .any(|l| l.text.starts_with("MCP client configured")));
    }
}
