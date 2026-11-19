---
id: 04-obsidian-vault-and-mcp
slug: /series/04-obsidian-vault-and-mcp
sidebar_position: 4
---

# Part 4: Obsidian Vault Compiler & MCP Server

In this tutorial, you compile a folder of Obsidian Markdown notes into a local SQLite WAL index using `nytheris-aether-vault`, run Dream consolidation on Daily Notes, inspect the loopback HTTP control plane, and query the vault over `stdio` with `aether-mcp`.

**Before you begin.** Complete [Part 1: Quickstart & Your First Memory](./01-quickstart-first-memory.md). Create a scratch directory (for example `/tmp/demo-vault`) or point at an existing Obsidian vault.

## Step 1: Author notes with Aether frontmatter and `.aetherignore`

Create three Markdown notes and one `.aetherignore` file inside `/tmp/demo-vault`:

1. `Profile/Preferences.md` (always loaded into `[AETHER_HOT_PROFILE]`):

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

2. `Arch/DB.md` (retrieved on demand via hybrid recall):

```markdown
---
aether_tier: warm
aether_scope: engineering
aether_kind: decision
---
# Schema
Use SQLite WAL mode with `TransactionBehavior::Immediate` for writes.
```

3. `Private/Journal.md` (blocked at the parse boundary):

```markdown
---
aether_tier: private
---
# Diary
Personal keys that never enter SQLite or MCP responses.
```

4. `.aetherignore` at the vault root:

```text
Finance/
*.secret.md
```

Built-in rules already skip `.obsidian`, `.git`, `.trash`, `.aether`, `Aether/Inbox.md`, `Aether/Compiled`, `.canvas`, `.excalidraw`, and `.excalidraw.md`.

## Step 2: Start `aether-mcp` with the loopback control plane

Run `aether-mcp` against `/tmp/demo-vault` and bind `ControlPlaneServer` to loopback port `4771`:

```bash
cargo run -p nytheris-aether-vault --bin aether-mcp -- \
  --vault /tmp/demo-vault \
  --watch-ms 2000 \
  --http-addr 127.0.0.1:4771
```

By default, the SQLite database lives at `<vault>/.aether/aether.sqlite`, and polling runs every `2000` ms (`--watch-ms 0` disables polling). Without `--require-auth`, `ControlPlaneServer::bind` rejects non-loopback addresses and accepts only `127.0.0.1` or `::1`.

## Step 3: Query the `stdio` MCP server

While `aether-mcp` waits on `stdin`, send newline-delimited JSON-RPC 2.0 messages (protocol `"2024-11-05"`). Send `initialize` first, then `tools/call`:

```json
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"demo","version":"0.1.0"}}}
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"aether_recall","arguments":{"query":"SQLite WAL","scope":"engineering","max_tokens":1500,"structured":true}}}
```

You see JSON containing both `content` and `structuredContent` with separated `static_prefix`, `dynamic_context`, `prefix_hash`, `static_prefix_tokens`, `dynamic_tokens`, and `tokens_used`. Notice that `Private/Journal.md` does not appear anywhere in the result.

Next, stage a new fact and inspect an indexed source section:

```json
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"aether_stage_memory","arguments":{"proposed_fact":"Set busy_timeout to 5000 ms on SQLite connections.","suggested_tier":"warm","scope":"engineering","target_note":"Arch/DB.md"}}}
{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"aether_inspect_source","arguments":{"source_pointer":"Arch/DB.md#Schema"}}}
```

`aether_stage_memory` returns a `candidate_id` (`cand_<12-hex>`) and writes the pending checkbox entry into `Aether/Inbox.md`. `aether_inspect_source` returns the verbatim `# Schema` Markdown section directly from disk, and rejects `Private/Journal.md#Diary` or `../../` traversal paths.

### Configure an MCP host client

Point an MCP-compatible IDE or agent client at the compiled `aether-mcp` binary:

```json
{
  "mcpServers": {
    "aether-vault": {
      "command": "/path/to/target/release/aether-mcp",
      "args": ["--vault", "/tmp/demo-vault", "--watch-ms", "2000", "--http-addr", "127.0.0.1:4771"]
    }
  }
}
```

You can also pass `AETHER_VAULT_PATH` and `AETHER_HTTP_ADDR` via environment variables instead of flags.

## Step 4: Approve Inbox items and inspect `Aether/Compiled/Approved.md`

Open `/tmp/demo-vault/Aether/Inbox.md`. You see the staged candidate formatted as a Markdown task:

```markdown
# Aether Memory Inbox
> Check `[x]` to approve and compile into active vault memory. Delete or mark `[-]` to reject.

- [ ] **[WARM | engineering]** Set busy_timeout to 5000 ms on SQLite connections.
  - `amu_candidate_id`: `cand_<12-hex>`
  - `target_link`: [[Arch/DB.md]]
  - `proposed_by`: `mcp-agent` on `2026-11-18`
```

You can approve or reject candidates in two ways:

- Edit `Aether/Inbox.md` in Obsidian by changing `[ ]` to `[x]` (approve) or `[-]` (reject).
- Send `POST /v1/inbox/decide` to the loopback HTTP control plane:

```bash
curl -sS http://127.0.0.1:4771/v1/inbox
curl -sS -X POST http://127.0.0.1:4771/v1/inbox/decide \
  -H 'Content-Type: application/json' \
  -d '{"candidate_id":"cand_<12-hex>","decision":"approve"}'
```

When approved, `VaultEngine` compiles the candidate into an active AMU, writes a `## cand_<12-hex>` section into `Aether/Compiled/Approved.md`, and removes it from `Aether/Inbox.md`. Call `aether_inspect_source` with `source_pointer: "Aether/Compiled/Approved.md#cand_<12-hex>"` to retrieve its verbatim provenance entry.

## Step 5: Run Daily Note Dream consolidation (`run_dream_pass`)

Create `Daily/2026-11-18.md` with a decision that replaces an older note:

```markdown
---
aether_scope: engineering
---
# Daily Log
## Decisions
- Decision: Switched local cache from Redis to SQLite WAL in [[Arch/DB]].
```

Trigger a Dream pass over HTTP (or call `VaultEngine::run_dream_pass()` from Rust):

```bash
curl -sS -X POST http://127.0.0.1:4771/v1/dream
```

You receive a JSON `DreamReport` showing `daily_notes_scanned`, `daily_notes_skipped_unchanged`, `candidates_staged`, `contradictions_flagged`, and `staged_candidate_ids`. The worker records a SHA-256 watermark in `dream_watermarks` so unchanged Daily Notes are skipped on later passes, and sets `supersedes_amu_id` when replacement cues (`switched`, `migrated`, `replaced`, `instead of`, `no longer`, `deprecated`, `superseded`) match an active AMU.

Check overall vault health anytime with `GET http://127.0.0.1:4771/v1/status`.

## Step 6: Push notes directly from Obsidian (`POST /v1/vault/push`)

When Obsidian runs separately from `aether-mcp` (or when `obsidian-aether-plugin` handles live note sync), the plugin pulls the server's indexed SHA-256 manifest via `GET /v1/vault/pull`, filters `aether_tier: private` and ignored notes client-side, pushes changed notes and deletions to `POST /v1/vault/push`, and writes the returned `Aether/Inbox.md` and `Aether/Compiled/Approved.md` snapshots back into Obsidian:

```bash
curl -sS http://127.0.0.1:4771/v1/vault/pull
curl -sS -X POST http://127.0.0.1:4771/v1/vault/push \
  -H 'Content-Type: application/json' \
  -d '{
    "notes": [{"path": "Arch/DB.md", "content": "---\naether_tier: warm\naether_scope: engineering\n---\n# Schema\nUse SQLite WAL mode.\n"}],
    "private_paths": ["Private/Journal.md"]
  }'
```

AI agents connected over MCP (`aether_recall`, `aether_inspect_source`, `aether_stage_memory`) immediately query those compiled memories without direct access to the user's Obsidian process.

**Next in series.** Continue to [Part 5: Team Git Sync, Scoped Auth & GPU KV Warming](./05-team-git-auth-and-kv-warming.md).
