# Aether Memory Companion

Aether Memory Companion connects Obsidian Desktop to an `aether-mcp` vault server so your notes stay synchronized with your agent memory index while private notes remain on disk.

The plugin requires a running `aether-mcp` control-plane server. You can run `aether-mcp` locally over loopback HTTP (`http://127.0.0.1:14771`) or connect to a remote team gateway over HTTPS with a bearer API key.

## Installation

You can install the plugin from Obsidian Community Plugins, through BRAT, manually from a GitHub release, or with the `aether-mcp` CLI.

### Community plugins

1. Open **Settings** in Obsidian and select **Community plugins**.
2. Turn off **Restricted mode** if prompted.
3. Select **Browse** and search for **Aether Memory Companion**.
4. Select **Install**, then turn on **Aether Memory Companion**.

### BRAT

1. Install and enable the **Obsidian42 BRAT** plugin.
2. Open the **BRAT** plugin settings and select **Add beta plugin**.
3. Enter the dedicated plugin repository (`https://github.com/chogerlate/aether-memory` or your mirrored plugin repository with `manifest.json` at its root) and add the plugin from release assets.
4. Enable **Aether Memory Companion** under **Community plugins**.

### Manual release installation

1. Download `main.js`, `manifest.json`, and `styles.css` from a release on GitHub.
2. Create the directory `<vault>/.obsidian/plugins/aether-memory/`.
3. Copy `main.js`, `manifest.json`, and `styles.css` into that directory.
4. Reload Obsidian and enable **Aether Memory Companion** under **Community plugins**.

### CLI setup with `aether-mcp --init`

Run `aether-mcp` with `--init` after building the plugin locally to install the bundle into your vault and print an `mcp.json` configuration snippet.

```bash
aether-mcp --init --vault /path/to/vault --http-addr 127.0.0.1:14771
```

Start the HTTP control plane before syncing in Obsidian.

```bash
aether-mcp --vault /path/to/vault --http-addr 127.0.0.1:14771 --http-only
```

## Configuration

Open **Settings** and select **Aether Memory Companion** to configure the connection, sync behavior, and KV-cache warming target. The settings tab stores values in `AetherPluginSettings`.

### Connection and authentication

- **Aether control-plane URL.** Sets `endpointUrl` (default `http://127.0.0.1:14771`). Accepts loopback `http://` URLs (`127.0.0.1`, `localhost`, `::1`) or remote `https://` endpoints for `aether-mcp --http-addr`.
- **API key.** Sets `apiKey` (default empty) using a masked password input. Remote hosts and loopback `https://` endpoints require a bearer token (`aek_...`).
- **Test connection.** Runs `GET /v1/vault/pull` and shows a notice with the number of indexed notes visible to your token.

### Live sync and lattice

- **Live vault sync.** Sets `liveSyncEnabled` (default `true`). Automatically pushes note creates, edits, deletions, and renames after Obsidian finishes loading the workspace layout.
- **Sync debounce (ms).** Sets `syncDebounceMs` (default `1500`, range `500` to `10000`). Delays rapid create and modify pushes per path while executing deletions and renames immediately in order.
- **Auto-pull interval (seconds).** Sets `autoPullIntervalSec` (default `0`). Polls the server in the background to refresh `Aether/Inbox.md` and `Aether/Compiled/Approved.md`, or disables polling when set to `0`.
- **Run Dream consolidation on push.** Sets `runDreamOnPush` (default `false`). Requests Dream consolidation during manual vault sync. Requires an Admin-scoped API key when `aether-mcp` enforces authentication.

### Inference and KV warming

- **KV endpoint (optional).** Sets `kvEndpoint` (default empty). Records the inference endpoint associated with KV-cache warming telemetry.
- **Default KV model.** Sets `kvModel` (default empty in settings, falling back to `llama-3.1-8b` in `AetherCompanionPlugin`). Sets the `model_id` parameter sent by the **Warm GPU KV Cache** command.

## Obsidian commands

The plugin registers five commands in the Obsidian Command Palette and updates the status bar between `Aether: Idle`, `Aether: Syncing…`, `Aether: Synced (HH:mm)`, and `Aether: Offline`. Clicking the status bar item triggers a vault sync.

- **Open Aether Inbox Review** (`aether-open-inbox-review`). Calls `GET /v1/inbox` and opens the **Aether Memory Inbox** pane in the right sidebar with **Approve** and **Reject** buttons.
- **Trigger Dream Pass** (`aether-trigger-dream-pass`). Calls `POST /v1/dream`, refreshes local mirror notes and pending Inbox counts, and displays a summary notice.
- **Sync Vault with Aether** (`aether-sync-vault`). Pulls the server manifest (`GET /v1/vault/pull`), hashes local Markdown files with SHA-256, pushes changed public notes (`POST /v1/vault/push`), and writes updated `Aether/Inbox.md` and `Aether/Compiled/Approved.md` files back to the vault.
- **Git Sync Team Vault** (`aether-git-sync`). Calls `POST /v1/git/sync` to fetch, merge, and reconcile vault and Aether lattice conflicts, then refreshes local mirror notes.
- **Warm GPU KV Cache** (`aether-warm-kv-cache`). Calls `POST /v1/kv/warm` for the configured default KV model and reports warmed blocks, tokens, and estimated time-to-first-token reduction.

## Frontmatter and exclusions

You control which notes sync to `aether-mcp` with YAML frontmatter, ignore files, and built-in path rules.

### Frontmatter

Add `aether_tier: private` in YAML frontmatter to keep a note exclusively on disk.

```yaml
---
aether_tier: private
---
```

When `isPrivateMarkdownNote` detects `aether_tier: private`, the plugin omits the note body from all HTTP requests and sends only the relative path in `private_paths` and `deleted_paths` so `aether-mcp` drops any earlier copy from its index. Notes without `private` may also set `aether_tier` (`hot`, `warm`, `cold`), `aether_scope`, `aether_kind`, `aether_status`, `valid_from`, `superseded_by`, and `tags` for `aether-mcp` indexing.

### `.aetherignore` and `.aetherhot`

Create `.aetherignore` at the vault root with newline-separated path rules, directory rules ending with `/`, or `*` and `?` globs. The parser skips blank lines and `#` comments. Editing or deleting `.aetherignore` triggers a delta sync so `aether-mcp` removes excluded paths from its index.

Create `.aetherhot` at the vault root using the same pattern syntax to promote matching notes into the hot profile tier on `aether-mcp` when frontmatter does not specify an explicit `aether_tier`.

### Built-in exclusions

`isIgnoredVaultPath` skips these paths before reading or uploading file contents.

- Non-Markdown files.
- Any path containing `.obsidian`, `.git`, `.trash`, or `.aether`.
- Vault mirror and plugin state prefixes `Aether/Inbox.md`, `Aether/Compiled`, and `.obsidian/plugins/aether-memory/data.json`.
- Drawing files ending with `.canvas`, `.excalidraw`, or `.excalidraw.md`.

## Privacy and network disclosure

- **Configured endpoint only.** Outbound HTTP requests go strictly to your configured `endpointUrl` through Obsidian's `requestUrl` API. The plugin sends zero telemetry, crash reports, or third-party requests.
- **Client-side private note filtering.** The plugin evaluates `aether_tier: private` frontmatter, `.aetherignore` rules, and built-in exclusions locally before building `POST /v1/vault/push` requests. Excluded note bodies never leave your disk.
- **Transport policy.** `assertControlPlaneBaseUrl` rejects non-loopback plain `http://` URLs by default and requires an API key for any non-loopback endpoint or loopback `https://` endpoint.
- **Local credential storage.** Obsidian stores your `apiKey` and plugin configuration locally in `<vault>/.obsidian/plugins/aether-memory/data.json` through `saveData`. Keep `.obsidian/plugins/aether-memory/data.json` out of version control.

## Development

Install dependencies, run unit tests, check types, build the production bundle `main.js`, and verify release compliance from `obsidian-aether-plugin/`.

```bash
npm ci
npm test
npm run typecheck
npm run build
npm run verify:release
```

To verify a tagged release archive against a specific SemVer release tag, pass `--tag` without a `v` prefix.

```bash
node scripts/verify-release.mjs --tag 0.4.0
```
