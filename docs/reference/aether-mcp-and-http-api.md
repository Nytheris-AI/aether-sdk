# aether-mcp CLI, MCP Tools, HTTP API & Frontmatter

This reference documents the `aether-mcp` binary CLI flags, MCP JSON-RPC 2.0 tool schemas, all 19 `ControlPlaneServer` HTTP endpoints, Obsidian YAML frontmatter keys, `.aetherignore` and `.aetherhot` rules, and the `obsidian-aether-plugin` TypeScript API.

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
| `ghcr.io/nytheris-ai/aether-mcp` | Multi-arch (`linux/amd64`, `linux/arm64`) container image with `ENTRYPOINT ["aether-mcp"]` and `/vault` volume |

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

## HTTP Control Plane & Team API (19 endpoints)

When `require_auth` is `false`, `ControlPlaneServer::bind` permits only `127.0.0.1` and `::1`. When `require_auth` is `true`, every request (except `OPTIONS` CORS preflight returning `204 No Content` and `GET /v1/openapi.json`) requires `Authorization: Bearer aek_<hex>`. Errors return `{ "error": "..." }` with status `400`, `401`, `403`, `404`, or `500`.

| Method | Path | Minimum role (when authenticated) | Request body or query | Response fields |
| --- | --- | --- | --- | --- |
| `GET` | `/v1/openapi.json` | Public (unauthenticated) | None | OpenAPI `3.1.0` specification (`bearerAuth`, `/v1/status`, `/v1/recall`, `/v1/stage`, `/v1/inspect`) |
| `GET` | `/v1/status` | Valid token (`Reader+`) | None | `VaultStatusReport` (`status`, `schema_version`, `embedder_id`, `embedding_dim`, `active_amus`, `pending_inbox_count`, `approved_inbox_count`, `hot_tokens_used`, `hot_token_budget`, `prefix_hash`) |
| `GET` | `/v1/inbox` | `Reader` | None | `inbox_path`, `approved_mirror_path`, `pending` (`Vec<InboxEntry>`) |
| `POST` | `/v1/inbox/decide` | `Admin` (plus write scope check) | `{ candidate_id, decision }` (`"approve"` or `"reject"`) | `{ candidate_id, decision, reconcile: InboxReconcileReport }` |
| `POST` | `/v1/inbox/reconcile` | `Admin` | None | `InboxReconcileReport` (`compiled`, `rejected`, `remaining_pending`) |
| `POST` | `/v1/dream` | `Admin` | None | `DreamReport` (`daily_notes_scanned`, `daily_notes_skipped_unchanged`, `candidates_staged`, `contradictions_flagged`, `staged_candidate_ids`) |
| `POST` | `/v1/sync` | `Admin` | None | `SyncReport` (`files_scanned`, `files_skipped_unchanged`, `files_skipped_self_write`, `files_updated`, `files_removed`, `private_skipped`, `amus_indexed`, `inbox_approved`, `inbox_rejected`, `reindexed_amus`) |
| `POST` | `/v1/git/sync` | `Admin` | Optional `{ remote, branch, push, commit_message }` | `GitSyncReport` (`remote`, `branch`, `head_commit`, `merged_cleanly`, `aether_conflicts_resolved`, `conflicted_files`, `sync_report`, `pushed`) |
| `GET` | `/v1/vault/pull` | `Reader` | None | `VaultPullResponse` (`manifest`, `inbox_path`, `inbox_markdown`, `approved_mirror_path`, `approved_mirror_markdown`) |
| `POST` | `/v1/vault/push` | `Contributor` (`Admin` for Inbox decisions or `run_dream`) | `{ notes?, deleted_paths?, private_paths?, inbox_markdown?, aetherignore?, run_dream? }` | `VaultPushResponse` (`sync_report`, `dream_report?`, `inbox_path`, `inbox_markdown`, `approved_mirror_path`, `approved_mirror_markdown`) |
| `POST` | `/v1/recall` | `Reader` | `{ query, scope?, max_tokens?, include_hot_profile?, kv_model? }` | `RecallResponse` (`formatted_context`, `hot_prefix`, `static_prefix`, `dynamic_context`, `hot_amus`, `warm_cold_amus`, `entries`, `static_prefix_tokens`, `dynamic_tokens`, `tokens_used`, `prefix_hash`, `kv_hint?`) |
| `POST` | `/v1/stage` | `Contributor` | `{ proposed_fact, suggested_tier?, scope?, target_note?, supersedes_amu_id?, proposed_by? }` | `{ candidate_id, inbox_path }` |
| `POST` | `/v1/inspect` | `Reader` | `{ source_pointer }` | `{ source_pointer, content }` |
| `POST` | `/v1/mcp` | Tool role (`Reader` or `Contributor`) | JSON-RPC 2.0 request object | JSON-RPC 2.0 response object |
| `POST` | `/v1/kv/warm` | `Contributor` | `{ model_id, scope?, amu_ids?, target_tier?, include_hot_prefix?, include_cache_blend?, block_tokens? }` | `KvWarmReport` (`model_id`, `target_tier`, `blocks_warmed`, `blocks_reused`, `tokens_warmed`, `estimated_baseline_ttft_ms`, `estimated_warmed_ttft_ms`, `estimated_ttft_saved_ms`, `ttft_reduction_pct`, `fallback_used`, `block_hashes`) |
| `GET` | `/v1/kv/stats` | Valid token (`Reader+`) | Optional `?model_id=<id>` | `KvCacheStats` (`adapter_id`, `persisted_blocks`, `total_tokens`, `total_bytes`, `tier_usage`, `models`) |
| `GET` | `/v1/auth/keys` | `Admin` | None | `{ keys: Vec<ScopedApiKey> }` |
| `POST` | `/v1/auth/keys` | `Admin` | `{ agent_id, role, scopes, expires_at? }` (RFC 3339) | `{ key: ScopedApiKey, secret: "aek_<64-hex>" }` |
| `POST` | `/v1/auth/keys/revoke` | `Admin` | `{ key_id }` | `{ key_id, revoked: bool }` |

## Obsidian YAML frontmatter, `.aetherignore`, and `.aetherhot`

### YAML frontmatter keys

| Key | Accepted values | Default |
| --- | --- | --- |
| `aether_tier` | `hot`, `warm`, `cold`, `private` | `hot` if matched by `.aetherhot`, else `warm` |
| `aether_scope` | Scalar, comma-separated, `[a, b]`, or YAML list of `ScopeTag` | `global` |
| `aether_kind` | `profile`, `rule`, `decision`, `spec`, `fleeting` | `fleeting` under `Daily/` or `Journal/`, `spec` elsewhere |
| `aether_status` | `active`, `superseded`, `archived` | `active` |
| `valid_from` | RFC 3339 timestamp or `YYYY-MM-DD` date | Indexing timestamp |
| `superseded_by` | `amu_<32-hex>` or UUID | `None` |
| `tags` | Scalar, `[a, b]`, or YAML list of tags (`#` stripped) | Empty |

### Built-in exclusions, `.aetherignore`, and `.aetherhot`

Built-in rules skip paths containing `.obsidian`, `.git`, `.trash`, or `.aether` segments, prefixes `Aether/Inbox.md` and `Aether/Compiled` (except `Aether/Compiled/Approved.md` during `inspect_source`), and extensions `.canvas`, `.excalidraw`, and `.excalidraw.md`.

Place a `.aetherignore` file at the vault root to add newline-separated ignore rules. Blank lines and `#` comments are ignored. Trailing `/` matches directories, `*` and `?` match globs, and paths with `/` match from the vault root.

Place a `.aetherhot` file at the vault root using the same path/glob syntax to promote matching notes into `hot` (`[AETHER_HOT_PROFILE]`) when YAML frontmatter does not specify an explicit `aether_tier`. Explicit frontmatter (`private`, `warm`, `cold`, `hot`) always takes precedence.

## `obsidian-aether-plugin` options and commands

`AetherControlClient` and `AetherCompanionPlugin` (default base URL `http://127.0.0.1:14771`) accept `AetherClientOptions`:

| Field | Type | Default | Behavior |
| --- | --- | --- | --- |
| `fetchImpl` | `typeof fetch` | Global `fetch` | Custom HTTP transport implementation |
| `apiKey` | `string \| undefined` | `undefined` | Required for non-loopback hosts and `https://` loopback URLs, sent as `Authorization: Bearer ...` |
| `allowInsecureRemoteHttp` | `boolean \| undefined` | `false` | Allows `http://` for non-loopback hosts, otherwise remote endpoints require `https://` |
| `defaultKvModel` | `string \| undefined` | `'llama-3.1-8b'` | Default `model_id` passed by `AetherCompanionPlugin::warmKvCache` |

### Registered Obsidian commands and vault adapter hooks

| Command ID | Command name | Action |
| --- | --- | --- |
| `aether-open-inbox-review` | `Open Aether inbox review` | Refreshes `GET /v1/inbox` and opens the 1-click approval pane |
| `aether-trigger-dream-pass` | `Trigger dream pass` | Calls `POST /v1/dream`, refreshes Inbox/mirrors and status bar, shows notice |
| `aether-sync-vault` | `Sync vault with Aether` | Calls `GET /v1/vault/pull` + `POST /v1/vault/push` with changed public notes, client-filtered `private_paths`, and deletions; writes updated `Inbox.md` and `Approved.md` back into Obsidian |
| `aether-git-sync` | `Git sync team vault` | Calls `POST /v1/git/sync`, refreshes Inbox/mirrors and status bar, shows notice |
| `aether-warm-kv-cache` | `Warm GPU KV cache` | Calls `POST /v1/kv/warm` for `defaultKvModel`, shows TTFT reduction notice |

`ObsidianHostAdapter` also accepts optional vault data hooks (`listMarkdownFiles`, `readFile`, `writeFile`, `onVaultEvent`) so `AetherCompanionPlugin` can stream note `create`, `modify`, `delete`, and `rename` events and synchronize `Aether/Inbox.md` and `Aether/Compiled/Approved.md` back into the user's Obsidian vault while filtering `aether_tier: private` notes locally.
