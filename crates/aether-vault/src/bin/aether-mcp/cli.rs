//! Command-line definition for `aether-mcp`, parsed with `clap`.
//!
//! Values are returned raw. `parse_cli_args_with_env` in the parent module applies the
//! environment fallbacks, validates them, and selects the run mode.

use std::path::PathBuf;

use clap::{ArgAction, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "aether-mcp",
    version,
    about = "Obsidian vault memory server for AI agents (MCP over stdio and an HTTP control plane)",
    disable_help_subcommand = true
)]
pub struct RawCli {
    /// Root directory of the Obsidian vault [env: AETHER_VAULT_PATH]
    #[arg(long, global = true, value_name = "PATH")]
    pub vault: Option<PathBuf>,

    /// SQLite index database path [default: <vault>/.aether/aether.sqlite]
    #[arg(long, global = true, value_name = "PATH")]
    pub db: Option<PathBuf>,

    /// Vault polling interval in milliseconds, 0 disables polling [default: 2000]
    #[arg(long, global = true, value_name = "MS")]
    pub watch_ms: Option<u64>,

    /// Address for the HTTP control plane, for example 127.0.0.1:14771 [env: AETHER_HTTP_ADDR]
    #[arg(long, global = true, value_name = "ADDR")]
    pub http_addr: Option<String>,

    /// Serve only the HTTP control plane until Ctrl-C or SIGTERM (requires --http-addr)
    #[arg(long, global = true, action = ArgAction::SetTrue)]
    pub http_only: bool,

    /// Install the Obsidian plugin files, write data.json, print mcp.json, and exit
    #[arg(long, global = true, action = ArgAction::SetTrue)]
    pub init: bool,

    /// With --init, also write the server into detected MCP client configs
    #[arg(long, global = true, action = ArgAction::SetTrue)]
    pub install_clients: bool,

    /// Require a bearer API key on every request [env: AETHER_REQUIRE_AUTH]
    #[arg(long, global = true, action = ArgAction::SetTrue)]
    pub require_auth: bool,

    /// Ensure an Admin key exists for the bootstrap-admin agent [env: AETHER_BOOTSTRAP_ADMIN_KEY]
    #[arg(long, global = true, value_name = "KEY")]
    pub bootstrap_admin_key: Option<String>,

    /// Bind the stdio session, or the --init output, to this scoped key [env: AETHER_API_KEY]
    #[arg(long, global = true, value_name = "KEY")]
    pub api_key: Option<String>,

    /// Inference endpoint for GPU KV-cache warming [env: AETHER_KV_ENDPOINT]
    #[arg(long, global = true, value_name = "URL")]
    pub kv_endpoint: Option<String>,

    /// Default model id for KV-cache hints [env: AETHER_KV_MODEL]
    #[arg(long, global = true, value_name = "ID")]
    pub kv_model: Option<String>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Manage the background service (launchd, systemd --user, or Task Scheduler)
    Service {
        #[command(subcommand)]
        action: ServiceCmd,
    },
    /// Check the vault, the server binary, and detected MCP client configs
    Doctor,
}

#[derive(Debug, Clone, Copy, Subcommand)]
pub enum ServiceCmd {
    /// Register and start the service
    Install,
    /// Stop and remove the service
    Uninstall,
    /// Show the service status
    Status,
}
