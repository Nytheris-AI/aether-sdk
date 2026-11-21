---
sidebar_label: AMC plugin settings and commands
---

# AMC plugin settings and commands

Reference for **AMC** (Aether Memory Companion), plugin id `aether-memory`.
Obsidian prefixes every command with the plugin name, so **Sync vault with Aether** appears as **AMC: Sync vault with Aether** in the Command Palette.

## Settings

| Setting | Default | What it does |
| --- | --- | --- |
| **Aether control-plane URL** | `http://127.0.0.1:14771` | Where `aether-mcp` listens. Plain `http://` is accepted only for loopback hosts. Remote servers need `https://` and an API key. |
| **API key** | Empty | Bearer token (`aek_...`) for servers started with `--require-auth`. Stored in the plugin's `data.json`. |
| **Live vault sync** | On | Pushes edits, renames, and deletions after Obsidian finishes loading. |
| **Sync debounce (ms)** | `1500` | How long to wait after an edit before pushing. Accepts `500` to `10000`. |
| **Auto-pull interval (seconds)** | `0` | Background refresh of `Aether/Inbox.md` and `Aether/Compiled/Approved.md`. `0` turns it off. |
| **Run Dream consolidation on push** | Off | Runs Dream after each push. Needs an `Admin` key on an authenticated server. |
| **Enable GPU KV-cache warming** | Off | Shows the KV settings and the **Warm GPU KV cache** command. For people who run their own vLLM or SGLang server. |
| **KV endpoint (optional)** | Empty | Inference endpoint for KV-cache warming. |
| **Default KV model** | Empty | Model id sent to `POST /v1/kv/warm`, for example `llama-3.1-8b`. |

The **Connection** card at the top of the settings tab checks `GET /v1/vault/pull` when you open the tab and after you change the URL or key.
It shows one of **Connected to aether-mcp**, **Can't reach aether-mcp**, **The server rejected the API key**, or **Connection problem**.

## Commands

| Command ID | Name | What it does |
| --- | --- | --- |
| `aether-sync-vault` | **Sync vault with Aether** | Pulls the server state, pushes changed public notes and deletions, and writes the updated Inbox and Approved files back. |
| `aether-open-inbox-review` | **Open Aether inbox review** | Opens the inbox pane to approve or reject proposed memories. |
| `aether-trigger-dream-pass` | **Trigger dream pass** | Calls `POST /v1/dream` to extract candidate facts and flag contradictions from Daily Notes. |
| `aether-git-sync` | **Git sync team vault** | Calls `POST /v1/git/sync`. See [Share a vault with a team](../guides/team-and-remote-setup.md). |
| `aether-warm-kv-cache` | **Warm GPU KV cache** | Calls `POST /v1/kv/warm`. Only available when KV-cache warming is enabled. |

Plugin command IDs and settings keys are stable across releases, so hotkeys and saved settings keep working.

## Status bar

| Text | Meaning |
| --- | --- |
| `Aether: Idle` | Connected, nothing to review. |
| `Aether: Synced (HH:mm)` | The last sync succeeded at that time. |
| `Aether: N to review` | Agents proposed N memories. Click to open the inbox. |
| `Aether: Offline` | The server is unreachable. Your notes are untouched. |

## What the plugin sends

- Requests go only to the control-plane URL you configure, through Obsidian's `requestUrl`.
- Notes with `aether_tier: private`, paths matched by `.aetherignore`, and built-in exclusions are filtered on your device before any request is built.
- The plugin sends no telemetry and spawns no processes.
- It writes `Aether/Inbox.md`, `Aether/Compiled/Approved.md`, and its own `data.json` inside the vault.
