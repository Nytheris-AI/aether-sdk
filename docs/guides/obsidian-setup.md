# Set up Obsidian and an AI agent with Aether

Connect an Obsidian Desktop vault to `aether-mcp` and an MCP-compatible AI agent so both share compiled memory while `private` notes stay local.

## Prerequisites

Install these tools before you start:

- **Rust toolchain** (`cargo` and `rustc`) to build `aether-mcp`
- **Node.js 20 or later** (`node` and `npm`) to build `obsidian-aether-plugin`
- **Obsidian Desktop 1.5.0 or later** (mobile is not supported because `obsidian-aether-plugin` sets `isDesktopOnly` to `true`)

## Step 1: Build and start `aether-mcp`

Build the `aether-mcp` binary from the repository root:

```bash
cargo build --release -p nytheris-aether-vault --bin aether-mcp
```

Start the standalone HTTP daemon in local loopback mode against your Obsidian vault directory:

```bash
./target/release/aether-mcp \
  --vault /path/to/vault \
  --watch-ms 2000 \
  --http-addr 127.0.0.1:14771 \
  --http-only
```

`--http-only` keeps `ControlPlaneServer` alive until `Ctrl-C` or `SIGTERM` without keeping `stdin` open. `aether-mcp` creates its SQLite WAL index at `<vault>/.aether/aether.sqlite` and polls for disk changes every `2000` milliseconds. Without `--require-auth`, `ControlPlaneServer::bind` accepts only `127.0.0.1` or `::1`.

If you want to bind to a non-loopback address or require bearer tokens for a team setup, generate a bootstrap admin key and pass `--require-auth` with `--bootstrap-admin-key`:

```bash
export AETHER_BOOTSTRAP_ADMIN_KEY="aek_$(openssl rand -hex 32)"

./target/release/aether-mcp \
  --vault /path/to/vault \
  --watch-ms 2000 \
  --http-addr 0.0.0.0:4771 \
  --require-auth \
  --bootstrap-admin-key "$AETHER_BOOTSTRAP_ADMIN_KEY"
```

## Step 2: Build and install `obsidian-aether-plugin`

Build the plugin bundle from `obsidian-aether-plugin`:

```bash
cd obsidian-aether-plugin
npm run build
```

Create the plugin directory inside your vault and copy `main.js` and `manifest.json` into it (or run `aether-mcp --init` from the repository root to copy the plugin, write `data.json`, and print your `mcp.json` snippet in one step):

```bash
./target/release/aether-mcp --init --vault /path/to/vault --http-addr 127.0.0.1:14771
```

Enable the plugin in Obsidian Desktop:

1. Open `/path/to/vault` as a vault in Obsidian.
2. Open **Settings**, then select **Community plugins**.
3. Turn off Restricted Mode if Obsidian prompts you.
4. Enable **Aether Memory Companion**.

## Step 3: Configure plugin settings and run initial sync

Open **Settings** and select **Aether Memory Companion**. Configure these fields:

1. Set **Aether control-plane URL** to `http://127.0.0.1:14771` for local loopback mode, or to your `https://` gateway URL for a remote server.
2. Leave **API key** empty for loopback mode without authentication. For an authenticated server, paste your `aek_...` bearer token.
3. Click **Test Connection**. Obsidian calls `GET /v1/vault/pull` and shows a notice with the number of indexed notes visible to your token.
4. Keep **Live vault sync** turned on so Obsidian pushes edits, renames, and deletions after startup layout finishes.
5. Set **Sync debounce (ms)** between `500` and `10000` (default `1500`) to control how long the plugin waits after a note edit before pushing.
6. Set **Auto-pull interval (seconds)** to a positive number if you want background polling to refresh `Aether/Inbox.md` and `Aether/Compiled/Approved.md` (`0` disables polling).
7. Keep **Run Dream consolidation on push** turned off unless you use an `Admin` key on an authenticated server or run in local loopback mode.
8. Set **Default KV model** (for example `llama-3.1-8b`) if you plan to run **Warm GPU KV Cache**.

Run **Sync Vault with Aether** from the Obsidian Command Palette, or click the **Aether: Idle** status bar item. The status bar updates to `Aether: Synced (HH:mm)` and shows a notice with updated, indexed, removed, and private-skipped counts.

## Step 4: Configure your vault notes and `.aetherignore`

Add YAML frontmatter to control how `aether-mcp` tiers, scopes, and classifies each note:

- `aether_tier` accepts `hot`, `warm`, `cold`, or `private` (default `warm`). Notes marked `hot` load into `[AETHER_HOT_PROFILE]` on every recall call. Notes marked `private` never leave your vault.
- `aether_scope` accepts a single scope string, a comma-separated list, `[a, b]`, or a YAML list (default `global`).
- `aether_kind` accepts `profile`, `rule`, `decision`, `spec`, or `fleeting`.

For example, create a private note that stays off the server:

```markdown
---
aether_tier: private
---
# Personal Keys
Local credentials that stay on disk only.
```

Create `.aetherignore` at the vault root to exclude whole folders or filename globs:

```text
Finance/
*.secret.md
```

Built-in rules already skip `.obsidian`, `.git`, `.trash`, `.aether`, `Aether/Inbox.md`, `Aether/Compiled`, `.canvas`, `.excalidraw`, and `.excalidraw.md`.

To pin existing notes or folders into `[AETHER_HOT_PROFILE]` without editing their YAML frontmatter, create `.aetherhot` at the vault root using the same path/glob syntax:

```text
Memory/
Profile/*.md
```

Notes matched by `.aetherhot` promote to `hot` unless their frontmatter sets an explicit `aether_tier` (`private`, `warm`, `cold`, or `hot`).

To verify that `private` notes stay off the server, run **Sync Vault with Aether** and confirm the notice reports a non-zero `private-skipped` count. You can also query `POST /v1/inspect` or `aether_inspect_source` for the private note path and confirm the server rejects it.

## Step 5: Connect an AI agent

You can connect an MCP client over `stdio` or over HTTP and HTTPS.

### Connect over `stdio`

Add `aether-mcp` to your MCP client `mcpServers` configuration (for example in Claude Desktop, Cursor, or Antigravity):

```json
{
  "mcpServers": {
    "aether-vault": {
      "command": "/path/to/target/release/aether-mcp",
      "args": ["--vault", "/path/to/vault", "--watch-ms", "0"]
    }
  }
}
```

Pass `--watch-ms 0` when the HTTP daemon from Step 1 is already running. Both processes share `<vault>/.aether/aether.sqlite` over SQLite WAL without running duplicate background scans. If `--require-auth` is enabled on your vault, add `"--api-key"` and your scoped `aek_...` token to `args`.

### Connect over HTTP or HTTPS

HTTP-capable agents can send JSON-RPC 2.0 requests directly to `POST http://127.0.0.1:14771/v1/mcp`, or import `GET http://127.0.0.1:14771/v1/openapi.json` (OpenAPI `3.1.0`) into REST tool clients such as ChatGPT Custom GPT Actions or Open WebUI (`https://<host>/v1/openapi.json` with `Authorization: Bearer aek_...` when `--require-auth` is enabled).

### Use the three MCP tools

Once connected, your agent calls three tools:

- `aether_recall` retrieves the `[AETHER_HOT_PROFILE]` static prefix and ranked warm or cold context within `max_tokens`.
- `aether_inspect_source` reads a verbatim Markdown section from disk by `source_pointer` (for example `Arch/DB.md#Schema`) while blocking `private` notes and path traversal.
- `aether_stage_memory` writes a candidate fact (`cand_<12-hex>`) to `Aether/Inbox.md` for human review.

## Step 6: Review staged facts and run Dream consolidation

When an agent calls `aether_stage_memory`, run **Sync Vault with Aether** in Obsidian (or wait for **Auto-pull interval (seconds)**) to pull the updated `Aether/Inbox.md`.

Approve or reject staged facts in either of two ways:

- Run **Open Aether Inbox Review** from the Command Palette and click **Approve** or **Reject**.
- Open `Aether/Inbox.md` in Obsidian, change `[ ]` to `[x]` to approve or `[-]` to reject, and let **Live vault sync** push the file.

On approval, `VaultEngine` compiles the fact into an active AMU, appends a `## cand_<12-hex>` section to `Aether/Compiled/Approved.md`, removes the item from `Aether/Inbox.md`, and syncs both files back into Obsidian.

To extract new candidate facts and flag contradictions from Daily Notes, run **Trigger Dream Pass** from the Command Palette. Obsidian calls `POST /v1/dream`, updates `Aether/Inbox.md`, and shows a notice with scanned notes, staged candidates, and flagged contradictions.

## Troubleshooting

- **Port or non-loopback bind error on startup.** `ControlPlaneServer::bind` rejects any `--http-addr` outside `127.0.0.1` and `::1` unless you pass `--require-auth`. In Obsidian, `assertControlPlaneBaseUrl` rejects non-loopback `http://` URLs unless `allowInsecureRemoteHttp` is enabled, and rejects any remote host without an **API key**.
- **`Test Connection` fails.** Confirm `aether-mcp` is running with `--http-addr` matching **Aether control-plane URL**. For `https://` or non-loopback endpoints, verify that **API key** is non-empty and belongs to a key with the `Reader` role or higher.
- **`Run Dream consolidation on push` fails with HTTP 403.** When `--require-auth` is enabled, running Dream consolidation during `POST /v1/vault/push` requires an `Admin` key. Turn off **Run Dream consolidation on push** when using a `Contributor` key, or issue an `Admin` key.
- **Edits right after opening Obsidian do not sync.** **Live vault sync** registers file watchers inside Obsidian's `onLayoutReady` callback so initial vault loading does not flood the server. Edit the note again after Obsidian finishes loading, or run **Sync Vault with Aether** manually.
