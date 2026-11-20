---
id: 04-obsidian-vault-and-mcp
slug: /series/04-obsidian-vault-and-mcp
sidebar_position: 4
---

# Part 4: Obsidian Vault Compiler & MCP Server

In this tutorial, you build and run the `aether-mcp` vault server, install the **Aether Memory Companion** plugin inside Obsidian, connect an AI agent over MCP, and approve agent-proposed facts back into your vault.

**Before you begin.** Complete [Part 1: Quickstart & Your First Memory](./01-quickstart-first-memory.md). Create a scratch Obsidian vault directory such as `/tmp/demo-vault`, or use an existing local vault folder.

## Step 1: Author notes with Aether frontmatter and `.aetherignore`

Create three Markdown notes and one `.aetherignore` file inside `/tmp/demo-vault`:

1. `Profile/Preferences.md` loads into `[AETHER_HOT_PROFILE]` on every recall call:

```markdown
---
aether_tier: hot
aether_scope:
  - global
aether_kind: profile
aether_status: active
valid_from: 2026-11-18
tags:
  - workflow
---
# Preferences
Prefer strict TDD and micro commits.
```

2. `Arch/DB.md` compiles into warm memory retrieved by hybrid search:

```markdown
---
aether_tier: warm
aether_scope: engineering
aether_kind: decision
---
# Schema
Use SQLite WAL mode with `TransactionBehavior::Immediate` for writes.
```

3. `Private/Journal.md` stays excluded at the parse boundary:

```markdown
---
aether_tier: private
---
# Diary
Personal keys that never enter SQLite or MCP responses.
```

4. `.aetherignore` at the vault root excludes whole folders or filename globs:

```text
Finance/
*.secret.md
```

Built-in rules already skip `.obsidian`, `.git`, `.trash`, `.aether`, `Aether/Inbox.md`, `Aether/Compiled`, `.canvas`, `.excalidraw`, and `.excalidraw.md`.

5. *(Optional)* `.aetherhot` at the vault root pins existing folders or globs (for example `Memory/`) into `[AETHER_HOT_PROFILE]` without editing their YAML frontmatter.

## Step 2: Build and start `aether-mcp`

Compile the release binary and start the standalone HTTP daemon on `127.0.0.1:14771`:

```bash
cargo build --release -p nytheris-aether-vault --bin aether-mcp

./target/release/aether-mcp \
  --vault /tmp/demo-vault \
  --watch-ms 2000 \
  --http-addr 127.0.0.1:14771 \
  --http-only
```

Leave this terminal open while Obsidian and your AI agents run (`--http-only` keeps the server running until `Ctrl-C` or `SIGTERM` without needing `stdin` open). By default, `aether-mcp` stores the SQLite WAL database at `<vault>/.aether/aether.sqlite` and polls disk changes every `2000` ms (`--watch-ms 0` disables polling). Without `--require-auth`, `ControlPlaneServer::bind` rejects non-loopback addresses and accepts only `127.0.0.1` or `::1`.

## Step 3: Build and install the Obsidian desktop plugin

Build the CommonJS plugin bundle (`main.js`) from `obsidian-aether-plugin`, then install it into your vault with `--init` (or copy `main.js` and `manifest.json` manually):

```bash
cd obsidian-aether-plugin
npm run build
cd ..
./target/release/aether-mcp --init --vault /tmp/demo-vault --http-addr 127.0.0.1:14771
```

Now enable and connect the plugin in Obsidian:

1. Open `/tmp/demo-vault` as a vault in Obsidian.
2. Open **Settings**, open **Community plugins**, disable Restricted Mode if prompted, and enable **Aether Memory Companion**.
3. Open **Settings** and select **Aether Memory Companion**.
4. Confirm **Aether control-plane URL** is `http://127.0.0.1:14771` and leave **API key** empty for loopback mode.
5. Check the connection card at the top. It shows **Connected to aether-mcp** with the count of indexed notes. Select **Test connection** to check again.
6. Keep **Live vault sync** turned on so edits sync after Obsidian finishes loading its layout.
7. Run **Sync vault with Aether** from the Obsidian Command Palette (or click the **Aether: Idle** status bar item).

The status bar changes to `Aether: Syncing…` and then `Aether: Synced (HH:mm)`. A notice reports updated, indexed, removed, and private-skipped counts, confirming `Private/Journal.md` stayed off the server.

## Step 4: Connect an AI agent over MCP

Add `aether-mcp` to your MCP client configuration (such as Claude Desktop, Cursor, or Antigravity) using the compiled binary path and your vault directory:

```json
{
  "mcpServers": {
    "aether-vault": {
      "command": "/path/to/target/release/aether-mcp",
      "args": ["--vault", "/tmp/demo-vault", "--watch-ms", "0"]
    }
  }
}
```

Passing `--watch-ms 0` on the `stdio` client process lets it share `<vault>/.aether/aether.sqlite` over SQLite WAL without duplicating background scans when the HTTP daemon from Step 2 is already running. If you do not run a separate HTTP daemon, omit `--watch-ms 0` or add `--http-addr 127.0.0.1:14771` so the MCP host launches the full server.

HTTP-capable agents can also call `POST http://127.0.0.1:14771/v1/mcp` directly without spawning a second process, or import `GET http://127.0.0.1:14771/v1/openapi.json` (OpenAPI `3.1.0`) into REST action clients such as Custom GPT Actions or Open WebUI. Test all three MCP tools against the running server:

```bash
curl -sS -X POST http://127.0.0.1:14771/v1/mcp \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"aether_recall","arguments":{"query":"SQLite WAL","scope":"engineering","max_tokens":1500,"structured":true}}}'

curl -sS -X POST http://127.0.0.1:14771/v1/mcp \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"aether_stage_memory","arguments":{"proposed_fact":"Set busy_timeout to 5000 ms on SQLite connections.","suggested_tier":"warm","scope":"engineering","target_note":"Arch/DB.md"}}}'

curl -sS -X POST http://127.0.0.1:14771/v1/mcp \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"aether_inspect_source","arguments":{"source_pointer":"Arch/DB.md#Schema"}}}'
```

Here is what each tool returns:

- `aether_recall` returns `content` and `structuredContent` with separated `static_prefix`, `dynamic_context`, `prefix_hash`, `static_prefix_tokens`, `dynamic_tokens`, and `tokens_used`. Notice that `Private/Journal.md` does not appear anywhere in the result.
- `aether_stage_memory` returns a `candidate_id` (`cand_<12-hex>`) and writes the pending checkbox entry into `Aether/Inbox.md`.
- `aether_inspect_source` returns the verbatim `# Schema` Markdown section directly from disk, and rejects `Private/Journal.md#Diary` or `../../` traversal paths.

Once connected in your IDE or chat client, ask the agent to search your vault ("Recall our SQLite write rules from Aether"), verify a claim ("Inspect `Arch/DB.md#Schema` in full"), or remember a new decision ("Stage this SQLite `busy_timeout` rule into Aether Inbox for review").

## Step 5: Approve Inbox items inside Obsidian

Run **Sync vault with Aether** in Obsidian (or wait for the optional **Auto-pull interval** timer) and open `Aether/Inbox.md`. You see the agent's staged candidate formatted as a Markdown task:

```markdown
# Aether Memory Inbox
> Check `[x]` to approve and compile into active vault memory. Delete or mark `[-]` to reject.

- [ ] **[WARM | engineering]** Set busy_timeout to 5000 ms on SQLite connections.
  - `amu_candidate_id`: `cand_<12-hex>`
  - `target_link`: [[Arch/DB.md]]
  - `proposed_by`: `mcp-agent` on `2026-11-18`
```

Approve or reject candidates in Obsidian using either method:

- Run **Open Aether inbox review** from the Command Palette and click **Approve** or **Reject**.
- Edit `Aether/Inbox.md` directly in Obsidian by changing `[ ]` to `[x]` (approve) or `[-]` (reject).

On approval, `VaultEngine` compiles the candidate into an active AMU, writes a `## cand_<12-hex>` section into `Aether/Compiled/Approved.md`, removes the approved entry from `Aether/Inbox.md`, and syncs both files back into Obsidian. Call `aether_inspect_source` with `source_pointer: "Aether/Compiled/Approved.md#cand_<12-hex>"` to verify the compiled provenance entry.

## Step 6: Run Daily Note Dream consolidation

Create `Daily/2026-11-18.md` in Obsidian with a decision that replaces an older note:

```markdown
---
aether_scope: engineering
---
# Daily Log
## Decisions
- Decision: Switched local cache from Redis to SQLite WAL in [[Arch/DB]].
```

Run **Trigger dream pass** from the Obsidian Command Palette (or send `curl -sS -X POST http://127.0.0.1:14771/v1/dream`). Obsidian shows a notice with scanned notes, staged candidates, and flagged contradictions, and updates `Aether/Inbox.md` with any newly proposed facts. The worker records a SHA-256 watermark in `dream_watermarks` so unchanged Daily Notes are skipped on later passes, and sets `supersedes_amu_id` when replacement cues (`switched`, `migrated`, `replaced`, `instead of`, `no longer`, `deprecated`, `superseded`) match an active AMU.

Check overall vault health anytime with `GET http://127.0.0.1:14771/v1/status`.

**Next in series.** Continue to [Part 5: Team Git Sync, Scoped Auth & GPU KV Warming](./05-team-git-auth-and-kv-warming.md).
