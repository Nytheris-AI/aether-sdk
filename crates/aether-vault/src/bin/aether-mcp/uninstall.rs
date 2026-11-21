//! `aether-mcp uninstall`: removes what setup and `--init` put on this computer.
//!
//! It removes only the `aether-vault` entry from MCP client configs, so other servers and
//! settings in those files stay as they are.

use std::fs;

use muninn_aether_core::{AetherError, Result};
use serde_json::{Map, Value};

use super::ui::{Level, Line, Ui};
use super::{known_mcp_client_configs, write_file_atomic, CliArgs};

const SERVER_NAME: &str = "aether-vault";

/// Removes the `aether-vault` entry from every MCP client config that has one.
pub fn run_uninstall(_parsed: &CliArgs) -> Result<()> {
    let ui = Ui::detect();
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
    ui.print(&lines);
    ui.outro("Uninstall finished");
    Ok(())
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
