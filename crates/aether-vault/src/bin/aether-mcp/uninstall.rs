//! `aether-mcp uninstall`: removes what setup and `--init` put on this computer.
//!
//! It removes only the `aether-vault` entry from MCP client configs, so other servers and
//! settings in those files stay as they are.

use std::fs;
use std::path::Path;

use muninn_aether_core::{AetherError, Result};
use serde_json::{Map, Value};

use super::ui::{Level, Line, Ui};
use super::{known_mcp_client_configs, write_file_atomic, CliArgs, OBSIDIAN_PLUGIN_ID};

const SERVER_NAME: &str = "aether-vault";

/// Removes everything aether-mcp put on this computer, asking first unless `--yes` is given.
pub fn run_uninstall(parsed: &CliArgs) -> Result<()> {
    let ui = Ui::detect();
    if !parsed.assume_yes && !ui.fancy() {
        return Err(AetherError::InvalidInput(
            "uninstall asks for confirmation, so it needs an interactive terminal. Run it in a terminal, or pass --yes.".into(),
        ));
    }
    ui.intro("aether-mcp uninstall");
    if !parsed.assume_yes {
        let confirmed = cliclack::confirm(
            "Remove aether-mcp from this computer? It stops the background service and removes aether-vault from MCP clients.",
        )
        .initial_value(false)
        .interact()
        .map_err(|err| AetherError::InvalidInput(format!("uninstall was cancelled: {err}")))?;
        if !confirmed {
            ui.outro("Nothing was removed");
            return Ok(());
        }
    }

    let mut lines = Vec::new();
    if let Err(err) = super::run_service(super::ServiceAction::Uninstall, parsed) {
        lines.push(Line::new(Level::Fail, format!("Background service: {err}")));
    }
    lines.extend(remove_client_entries()?);
    lines.extend(remove_vault_files(parsed));
    ui.print(&lines);
    ui.note(
        "Still to do",
        "Remove the AMC plugin in Obsidian, and remove it from BRAT too if you installed it there.",
    );
    ui.outro("Uninstall finished");
    Ok(())
}

/// Removes the `aether-vault` entry from every MCP client config that has one.
fn remove_client_entries() -> Result<Vec<Line>> {
    let home = super::resolve_home_dir()?;
    let mut lines = Vec::new();
    for (name, _dir, config_path) in known_mcp_client_configs(&home) {
        if !config_path.is_file() {
            continue;
        }
        let outcome = fs::read_to_string(&config_path)
            .map_err(|err| AetherError::InvalidInput(format!("failed to read: {err}")))
            .and_then(|raw| remove_mcp_server_from_config(&raw, SERVER_NAME))
            .and_then(|updated| match updated {
                Some(rendered) => {
                    write_file_atomic(&config_path, rendered.as_bytes()).map(|()| true)
                }
                None => Ok(false),
            });
        match outcome {
            Ok(true) => lines.push(Line::new(
                Level::Ok,
                format!(
                    "Removed {SERVER_NAME} from {name}: {}",
                    config_path.display()
                ),
            )),
            Ok(false) => {}
            Err(err) => lines.push(Line::new(
                Level::Fail,
                format!("{name}: could not update {}: {err}", config_path.display()),
            )),
        }
    }
    if lines.is_empty() {
        lines.push(Line::new(
            Level::Info,
            "No MCP client had aether-vault configured",
        ));
    }
    Ok(lines)
}

/// Removes the vault's plugin files when `--vault` names it, and its index and Aether folder
/// with `--purge-vault`. Notes are never touched.
fn remove_vault_files(parsed: &CliArgs) -> Vec<Line> {
    if !parsed.vault_explicit {
        return vec![Line::new(
            Level::Info,
            "Vault left alone: pass --vault <path> to remove its plugin files",
        )
        .hint("add --purge-vault to remove its index and Aether folder too")];
    }
    let vault = &parsed.vault_root;
    let mut lines = Vec::new();
    match remove_vault_plugin(vault) {
        Ok(true) => lines.push(Line::new(
            Level::Ok,
            format!("Removed the Obsidian plugin files from {}", vault.display()),
        )),
        Ok(false) => lines.push(Line::new(Level::Info, "No Obsidian plugin files to remove")),
        Err(err) => lines.push(Line::new(
            Level::Fail,
            format!("Obsidian plugin files: {err}"),
        )),
    }
    if parsed.purge_vault {
        match purge_vault_data(vault) {
            Ok(true) => lines.push(Line::new(
                Level::Ok,
                format!(
                    "Removed the index and Aether folder from {}",
                    vault.display()
                ),
            )),
            Ok(false) => lines.push(Line::new(
                Level::Info,
                "No index or Aether folder to remove",
            )),
            Err(err) => lines.push(Line::new(Level::Fail, format!("Vault data: {err}"))),
        }
    }
    lines
}

/// Deletes the AMC plugin folder from the vault. Returns whether anything was removed.
pub fn remove_vault_plugin(vault: &Path) -> std::io::Result<bool> {
    remove_dir_if_exists(
        &vault
            .join(".obsidian")
            .join("plugins")
            .join(OBSIDIAN_PLUGIN_ID),
    )
}

/// Deletes the vault's SQLite index and `Aether/` folder. Notes are kept.
pub fn purge_vault_data(vault: &Path) -> std::io::Result<bool> {
    let index = remove_dir_if_exists(&vault.join(".aether"))?;
    let aether = remove_dir_if_exists(&vault.join("Aether"))?;
    Ok(index || aether)
}

fn remove_dir_if_exists(path: &Path) -> std::io::Result<bool> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err),
    }
}

/// Removes `server_name` from `mcpServers` in an MCP client config.
/// Returns `None` when there is no such entry, so the caller leaves the file untouched.
pub fn remove_mcp_server_from_config(raw: &str, server_name: &str) -> Result<Option<String>> {
    let mut root: Map<String, Value> = match serde_json::from_str::<Value>(raw)? {
        Value::Object(map) => map,
        _ => {
            return Err(AetherError::InvalidInput(
                "MCP config root must be a JSON object".into(),
            ));
        }
    };
    let removed = match root.get_mut("mcpServers") {
        Some(Value::Object(servers)) => servers.remove(server_name).is_some(),
        _ => false,
    };
    if !removed {
        return Ok(None);
    }
    let mut rendered = serde_json::to_string_pretty(&Value::Object(root))?;
    rendered.push('\n');
    Ok(Some(rendered))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A throwaway vault folder that is deleted when the test ends.
    struct TempVault(std::path::PathBuf);

    impl Drop for TempVault {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn temp_vault() -> TempVault {
        let root = std::env::temp_dir().join(format!("aether_uninstall_{}", uuid::Uuid::new_v4()));
        let plugin = root.join(".obsidian").join("plugins").join("aether-memory");
        std::fs::create_dir_all(&plugin).expect("plugin dir");
        std::fs::write(plugin.join("main.js"), "x").expect("plugin file");
        std::fs::write(root.join("note.md"), "keep me").expect("note");
        std::fs::create_dir_all(root.join(".aether")).expect("index dir");
        std::fs::write(root.join(".aether").join("aether.sqlite"), "x").expect("index");
        std::fs::create_dir_all(root.join("Aether")).expect("aether dir");
        TempVault(root)
    }

    #[test]
    fn remove_vault_plugin_deletes_the_plugin_folder_and_nothing_else() {
        let vault = temp_vault();
        assert!(remove_vault_plugin(&vault.0).expect("removes"));
        assert!(!vault.0.join(".obsidian/plugins/aether-memory").exists());
        assert!(vault.0.join("note.md").is_file());
        assert!(!remove_vault_plugin(&vault.0).expect("second run is a no-op"));
    }

    #[test]
    fn purge_vault_data_removes_index_and_aether_folder_but_keeps_notes() {
        let vault = temp_vault();
        assert!(purge_vault_data(&vault.0).expect("purges"));
        assert!(!vault.0.join(".aether").exists());
        assert!(!vault.0.join("Aether").exists());
        assert!(vault.0.join("note.md").is_file());
        assert!(!purge_vault_data(&vault.0).expect("second run is a no-op"));
    }

    #[test]
    fn removes_only_the_aether_vault_entry_and_keeps_other_servers() {
        let raw = r#"{"theme":"dark","mcpServers":{"aether-vault":{"command":"aether-mcp"},"other":{"command":"echo"}}}"#;
        let updated = remove_mcp_server_from_config(raw, "aether-vault")
            .expect("valid config")
            .expect("entry present");
        let parsed: serde_json::Value = serde_json::from_str(&updated).expect("json");
        assert_eq!(parsed["theme"], "dark");
        assert_eq!(parsed["mcpServers"]["other"]["command"], "echo");
        assert!(parsed["mcpServers"].get("aether-vault").is_none());
    }

    #[test]
    fn leaves_the_file_alone_when_there_is_nothing_to_remove() {
        assert_eq!(
            remove_mcp_server_from_config(r#"{"mcpServers":{"other":{}}}"#, "aether-vault")
                .expect("valid config"),
            None
        );
        assert_eq!(
            remove_mcp_server_from_config(r#"{"theme":"dark"}"#, "aether-vault")
                .expect("valid config"),
            None
        );
    }

    #[test]
    fn rejects_configs_that_are_not_json_objects() {
        assert!(remove_mcp_server_from_config("not json", "aether-vault").is_err());
        assert!(remove_mcp_server_from_config("[]", "aether-vault").is_err());
    }
}
