---
sidebar_position: 3
---

# Install AMC in Obsidian

**AMC** (Aether Memory Companion, plugin id `aether-memory`) syncs your vault to `aether-mcp` and lets you review the memories your agents propose.
It requires Obsidian Desktop 1.5.0 or later. Mobile is not supported.

## Install the plugin

The Obsidian Community plugins listing is pending review, so install through one of these methods for now.

- **BRAT (beta).** Install **Obsidian42 BRAT**, choose **Add beta plugin**, and enter `https://github.com/muninn-hq/aether-memory`.
- **From a local build.** If you built the plugin from `obsidian-aether-plugin/`, run `aether-mcp --init --install-clients --vault /path/to/vault --http-addr 127.0.0.1:14771`.
  It copies `main.js`, `manifest.json`, and `styles.css`, writes `data.json`, configures detected MCP clients, and prints an `mcp.json` snippet.

Then enable the plugin:

1. Open `/path/to/vault` as a vault in Obsidian.
2. Open **Settings**, then **Community plugins**.
3. Turn off Restricted Mode if Obsidian prompts you.
4. Enable **AMC**.

## Connect to the server

Open **Settings**, then **AMC**.

1. Leave **Aether control-plane URL** at `http://127.0.0.1:14771` for a local server, or enter your `https://` gateway URL for a remote one.
2. Leave **API key** empty for a local server without authentication.
   For an authenticated server, paste your `aek_...` token.
3. Read the connection card at the top.
   It should say **Connected to aether-mcp** and show how many notes are indexed.
   Select **Test connection** to check again.

The **Local server quickstart** buttons (**Copy install**, **Copy service install**, **Copy start**) copy ready-made commands for your vault.

## Choose sync behavior

| Setting | Recommendation |
| --- | --- |
| **Live vault sync** | Keep it on so edits, renames, and deletions are pushed after Obsidian finishes loading. |
| **Sync debounce (ms)** | Leave at `1500`, or pick `500` to `10000`. |
| **Auto-pull interval (seconds)** | Set a positive number if you want `Aether/Inbox.md` and `Aether/Compiled/Approved.md` to refresh in the background. |
| **Run Dream consolidation on push** | Leave off unless you use an `Admin` key or a local server. |
| **Enable GPU KV-cache warming** | Leave off unless you run your own vLLM or SGLang server. |

## Run the first sync

Run **Sync vault with Aether** from the Command Palette, or click the status bar item.
The status bar changes to `Aether: Synced (HH:mm)` and a notice such as "Synced: 2 notes updated, 1 private note kept on this device." appears.

When agents have proposed memories, the status bar shows `Aether: N to review`.
Click it to open the inbox.

See the full list of settings and commands in the [AMC plugin reference](../reference/amc-plugin.md).

## Next step

[Connect an AI agent](./connect-an-agent.md).
