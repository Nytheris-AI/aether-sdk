---
sidebar_label: aether-mcp CLI and MCP tools
---

# aether-mcp CLI and MCP tools

Every `aether-mcp` command-line flag, environment variable, and subcommand, plus the three MCP tools that AI agents call.
For the HTTP endpoints, see the [HTTP API reference](./http-api.md).
For step-by-step setup, start with [Install aether-mcp](../get-started/install-aether-mcp.md).

## `aether-mcp` CLI flags and environment variables

CLI flags override environment variables when both are set.

| CLI flag | Environment variable | Default | Description |
| --- | --- | --- | --- |
| `--vault <path>` | `AETHER_VAULT_PATH` | `.` | Root directory of the Obsidian Markdown vault |
| `--db <path>` | None | `<vault>/.aether/aether.sqlite` | SQLite WAL index database path |
| `--watch-ms <ms>` | None | `2000` | Vault polling interval in milliseconds (`0` disables polling) |
| `--http-addr <addr>` | `AETHER_HTTP_ADDR` | Disabled | Bind `SocketAddr` for `ControlPlaneServer` (e.g. `127.0.0.1:14771`) |
| `--http-only` | None | `false` | Serves `ControlPlaneServer` without `stdio` MCP until `Ctrl-C` / `SIGTERM` (requires `--http-addr`) |
| `--init` | None | `false` | Installs `obsidian-aether-plugin`, writes `data.json`, prints `mcp.json`, and exits (`AETHER_PLUGIN_DIR` overrides plugin source) |
| `--require-auth` | `AETHER_REQUIRE_AUTH` (`1`, `true`, `yes`) | `false` | Requires `Authorization: Bearer aek_<hex>` and allows non-loopback HTTP binds |
| `--bootstrap-admin-key <key>` | `AETHER_BOOTSTRAP_ADMIN_KEY` | None | Ensures an `Admin` key for `AgentId` `"bootstrap-admin"` exists in SQLite |
| `--api-key <key>` | `AETHER_API_KEY` | None | Binds the `stdio` MCP session (or `--init` `data.json` / `mcp.json`) to a validated `ScopedApiKey` |
| `--kv-endpoint <url>` | `AETHER_KV_ENDPOINT` | None | Attaches `VllmLmCacheHttpAdapter` targeting an `http://` inference endpoint |
| `--kv-model <id>` | `AETHER_KV_MODEL` | None | Sets `VaultConfig::default_kv_model` (`ModelId`) for recall `kv_hint` metadata |

### Subcommands and container image

| Command or image | Purpose |
| --- | --- |
| `aether-mcp service install [--vault <path>] [--http-addr <addr>] [--watch-ms <ms>]` | Registers and starts an auto-starting user service (`launchd` on macOS, `systemd --user` on Linux, Task Scheduler on Windows; defaults `--http-addr` to `127.0.0.1:14771`) |
| `aether-mcp service status` | Prints the status of the installed user service |
| `aether-mcp service uninstall` | Stops and removes the user service definition |
| `aether-mcp doctor [--vault <path>]` | Checks the vault, the binary, and detected MCP client configs |
| `aether-mcp completions <bash\|zsh\|fish\|powershell\|elvish>` | Prints a shell completion script to stdout |
| `aether-mcp --help`, `aether-mcp --version` | Prints usage or the version |
| `ghcr.io/muninn-hq/aether-mcp` | Multi-arch (`linux/amd64`, `linux/arm64`) container image with `ENTRYPOINT ["aether-mcp"]` and `/vault` volume |

## MCP JSON-RPC 2.0 protocol and tools

`McpServer` speaks newline-delimited JSON-RPC 2.0 (`protocolVersion` `"2024-11-05"`) over `stdio` and `POST /v1/mcp`. Supported methods are `initialize`, `ping`, `tools/list`, and `tools/call`. Notifications (messages without `id`) produce no response line.

### `aether_recall`

Scoped sessions require the `Reader` role or higher.

| Argument | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `query` | `string` | Yes | `n/a` | Natural language or keyword query (may be empty if `include_hot_profile` is `true`) |
| `scope` | `string` | No | Agent scopes or all | Filter `ScopeTag`, intersected with caller scopes on scoped sessions |
| `max_tokens` | `integer` | No | `1500` | Strict positive token ceiling across `static_prefix` and `dynamic_context` |
| `include_hot_profile` | `boolean` | No | `true` | Includes deterministic `[AETHER_HOT_PROFILE]` prefix block |
| `structured` | `boolean` | No | `false` | When `true`, `content[0].text` contains JSON instead of formatted text |
| `kv_model` | `string` | No | `default_kv_model` | Target `ModelId` for attaching `kv_hint` block reuse metadata |

`structuredContent` is always included on success with fields `static_prefix`, `dynamic_context`, `prefix_hash`, `static_prefix_tokens`, `dynamic_tokens`, `tokens_used`, and optional `kv_hint`.

### `aether_stage_memory`

Scoped sessions require the `Contributor` role or higher.

| Argument | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `proposed_fact` | `string` | Yes | `n/a` | Concise non-empty atomic statement to stage in `Aether/Inbox.md` |
| `suggested_tier` | `string` | No | `"warm"` | One of `"hot"`, `"warm"`, or `"cold"` |
| `scope` | `string` | No | `agent.default_write_scope()` or `"global"` | Target `ScopeTag` (must be permitted for scoped callers) |
| `target_note` | `string` | No | None | Suggested vault note path or wikilink |
| `supersedes_amu_id` | `string` | No | None | Optional `AmuId` (`amu_<32-hex>` or UUID) of an active AMU to supersede on approval |

Returns JSON `{ "candidate_id": "cand_<12-hex>", "inbox_path": "Aether/Inbox.md" }`.

### `aether_inspect_source`

Scoped sessions require the `Reader` role or higher.

| Argument | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `source_pointer` | `string` | Yes | `n/a` | Validated `SourcePointer` (for example `"Arch/DB.md#Schema"` or `"Aether/Compiled/Approved.md#cand_<hex>"`) |

Reads live verbatim Markdown from disk while enforcing traversal guards, `.aetherignore`, `aether_tier: private` rejection, and scoped read authorization.
