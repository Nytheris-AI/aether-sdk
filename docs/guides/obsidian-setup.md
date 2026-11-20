# Set up Obsidian and an AI agent with Aether

Connect an Obsidian Desktop vault to `aether-mcp` and an MCP-compatible AI agent so both share compiled memory while `private` notes stay local.

Want to see it first? Watch the [Obsidian and AI agent walkthrough](./obsidian-walkthrough.md), recorded from a real vault.

## Prerequisites

- **Obsidian Desktop 1.5.0 or later** (mobile is not supported because `aether-memory` sets `isDesktopOnly` to `true`)

For local use, you do **not** need Rust or Node.js installed — prebuilt `aether-mcp` binaries and the Obsidian plugin bundle are published on GitHub Releases.

## Step 1: Install and start `aether-mcp`

### Option A: One-line installer + background service (recommended for desktop)

Install the latest `aether-mcp` release binary:

```bash
# macOS / Linux
curl -fsSL https://raw.githubusercontent.com/muninn-hq/aether-sdk/main/scripts/install.sh | sh
```

```powershell
# Windows (PowerShell)
irm https://raw.githubusercontent.com/muninn-hq/aether-sdk/main/scripts/install.ps1 | iex
```

Register `aether-mcp` as an auto-starting background service for your Obsidian vault (`launchd` on macOS, `systemd --user` on Linux, or Task Scheduler on Windows):

```bash
aether-mcp service install --vault /path/to/vault
```

Check or remove the background service at any time:

```bash
aether-mcp service status
aether-mcp service uninstall
```

Or run `aether-mcp` in the foreground instead of a background service:

```bash
aether-mcp --vault /path/to/vault --watch-ms 2000 --http-addr 127.0.0.1:14771 --http-only
```

`--http-only` keeps `ControlPlaneServer` alive until `Ctrl-C` or `SIGTERM` without keeping `stdin` open. `aether-mcp` creates its SQLite WAL index at `<vault>/.aether/aether.sqlite` and polls for disk changes every `2000` milliseconds. Without `--require-auth`, `ControlPlaneServer::bind` accepts only `127.0.0.1` or `::1`.

### Option B: Docker (recommended for team or remote gateways)

Run the multi-arch container image from GitHub Container Registry with a mounted vault directory and bearer authentication:

```bash
export AETHER_BOOTSTRAP_ADMIN_KEY="aek_$(openssl rand -hex 32)"

docker run -d \
  --name aether-mcp \
  --user "$(id -u):$(id -g)" \
  -p 4771:4771 \
  -v /path/to/vault:/vault \
  ghcr.io/muninn-hq/aether-mcp:latest \
  --vault /vault \
  --watch-ms 2000 \
  --http-addr 0.0.0.0:4771 \
  --http-only \
  --require-auth \
  --bootstrap-admin-key "$AETHER_BOOTSTRAP_ADMIN_KEY"
```

Keep `--http-only` when you pass your own arguments.
Without it `aether-mcp` also starts its stdio MCP loop, which ends immediately in a detached container and stops the server.
`--user` makes the container write to your vault as you, so the files keep your ownership and permissions.

### Option C: Build from source (contributors)

If you are developing in the `aether-sdk` repository, build `aether-mcp` with Cargo and `obsidian-aether-plugin` with Node.js 20+:

```bash
cargo build --release -p muninn-aether-vault --bin aether-mcp
```

## Step 2: Install the Obsidian plugin (`aether-memory`)

Install **AMC** (Aether Memory Companion) in Obsidian Desktop using either of these methods (the Obsidian Community plugins directory listing is pending review):

- **BRAT (beta)**: Install **Obsidian42 BRAT**, select **Add beta plugin**, and enter `https://github.com/muninn-hq/aether-memory`.
- **CLI (`--init`)**: If you built the plugin locally from `obsidian-aether-plugin/`, run `aether-mcp --init --install-clients --vault /path/to/vault --http-addr 127.0.0.1:14771` to copy `main.js`, `manifest.json`, and `styles.css`, write `data.json`, auto-configure detected MCP clients (Cursor, Claude Desktop, Claude Code, Antigravity), and print your `mcp.json` snippet. Run `aether-mcp doctor --vault /path/to/vault` to verify the vault and client setup.

Enable the plugin in Obsidian Desktop:

1. Open `/path/to/vault` as a vault in Obsidian.
2. Open **Settings**, then select **Community plugins**.
3. Turn off Restricted Mode if Obsidian prompts you.
4. Enable **AMC**.

## Step 3: Configure plugin settings and run initial sync

Open **Settings** and select **AMC**. You can use the **Local server quickstart** buttons (**Copy install**, **Copy service install**, and **Copy start**) to copy pre-filled commands for your vault, then configure these fields:

1. Set **Aether control-plane URL** to `http://127.0.0.1:14771` for local loopback mode, or to your `https://` gateway URL for a remote server.
2. Leave **API key** empty for loopback mode without authentication. For an authenticated server, paste your `aek_...` bearer token.
3. Check the connection card at the top of the settings tab. It tests `GET /v1/vault/pull` when you open the tab and after you change the URL or API key, and shows **Connected to aether-mcp** with the number of indexed notes. Select **Test connection** to check again.
4. Keep **Live vault sync** turned on so Obsidian pushes edits, renames, and deletions after startup layout finishes.
5. Set **Sync debounce (ms)** between `500` and `10000` (default `1500`) to control how long the plugin waits after a note edit before pushing.
6. Set **Auto-pull interval (seconds)** to a positive number if you want background polling to refresh `Aether/Inbox.md` and `Aether/Compiled/Approved.md` (`0` disables polling).
7. Keep **Run Dream consolidation on push** turned off unless you use an `Admin` key on an authenticated server or run in local loopback mode.
8. Optional: turn on **Enable GPU KV-cache warming** if you run your own vLLM or SGLang server, then set **Default KV model** (for example `llama-3.1-8b`) to use **Warm GPU KV cache**.

Run **Sync vault with Aether** from the Obsidian Command Palette, or click the **Aether: Idle** status bar item. The status bar updates to `Aether: Synced (HH:mm)` and shows a notice such as "Synced: 2 notes updated, 1 private note kept on this device." When AI agents have proposed memories, the status bar shows `Aether: N to review` instead, and clicking it opens the inbox.

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

To verify that `private` notes stay off the server, run **Sync vault with Aether** and confirm the notice reports a non-zero `private-skipped` count. You can also query `POST /v1/inspect` or `aether_inspect_source` for the private note path and confirm the server rejects it.

## Step 5: Connect an AI agent

You can connect an MCP client over `stdio` or over HTTP and HTTPS.

### Connect over `stdio`

Add `aether-mcp` to your MCP client `mcpServers` configuration (for example in Claude Desktop, Cursor, or Antigravity):

```json
{
  "mcpServers": {
    "aether-vault": {
      "command": "aether-mcp",
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

When an agent calls `aether_stage_memory`, run **Sync vault with Aether** in Obsidian (or wait for **Auto-pull interval (seconds)**) to pull the updated `Aether/Inbox.md`.

Approve or reject staged facts in either of two ways:

- Run **Open Aether inbox review** from the Command Palette and click **Approve** or **Reject**.
- Open `Aether/Inbox.md` in Obsidian, change `[ ]` to `[x]` to approve or `[-]` to reject, and let **Live vault sync** push the file.

On approval, `VaultEngine` compiles the fact into an active AMU, appends a `## cand_<12-hex>` section to `Aether/Compiled/Approved.md`, removes the item from `Aether/Inbox.md`, and syncs both files back into Obsidian.

To extract new candidate facts and flag contradictions from Daily Notes, run **Trigger dream pass** from the Command Palette. Obsidian calls `POST /v1/dream`, updates `Aether/Inbox.md`, and shows a notice with scanned notes, staged candidates, and flagged contradictions.

## Troubleshooting

- **Port or non-loopback bind error on startup.** `ControlPlaneServer::bind` rejects any `--http-addr` outside `127.0.0.1` and `::1` unless you pass `--require-auth`. In Obsidian, `assertControlPlaneBaseUrl` rejects non-loopback `http://` URLs unless `allowInsecureRemoteHttp` is enabled, and rejects any remote host without an **API key**.
- **`Test connection` fails.** Confirm `aether-mcp` is running with `--http-addr` matching **Aether control-plane URL**. For `https://` or non-loopback endpoints, verify that **API key** is non-empty and belongs to a key with the `Reader` role or higher.
- **`Run Dream consolidation on push` fails with HTTP 403.** When `--require-auth` is enabled, running Dream consolidation during `POST /v1/vault/push` requires an `Admin` key. Turn off **Run Dream consolidation on push** when using a `Contributor` key, or issue an `Admin` key.
- **Edits right after opening Obsidian do not sync.** **Live vault sync** registers file watchers inside Obsidian's `onLayoutReady` callback so initial vault loading does not flood the server. Edit the note again after Obsidian finishes loading, or run **Sync vault with Aether** manually.
