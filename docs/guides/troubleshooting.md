---
sidebar_position: 5
---

# Troubleshooting

## The connection card says "Can't reach aether-mcp"

The server is not running, or the URL is wrong.

- Run `aether-mcp service status`, or start it in the foreground with the command in [Install aether-mcp](../get-started/install-aether-mcp.md).
- Confirm **Aether control-plane URL** matches the server's `--http-addr`. The default is `http://127.0.0.1:14771`.
- The status bar shows `Aether: Offline` in this state. Your notes are untouched. Start the server and run **Sync vault with Aether** to recover.

## The connection card says "The server rejected the API key"

The server was started with `--require-auth`.
Paste a valid `aek_...` token into **API key**.
The key needs the `Reader` role or higher.

## The server refuses to start on a non-loopback address

Without `--require-auth`, the server accepts only `127.0.0.1` and `::1`.
Add `--require-auth` and a key, or bind to loopback.
AMC also rejects plain `http://` URLs for non-loopback hosts and any remote host without an API key.

## A Docker container exits immediately

Keep `--http-only` in the arguments.
Without it the stdio loop ends in a detached container and stops the server.

## Run Dream consolidation on push fails with HTTP 403

On an authenticated server, this needs an `Admin` key.
Turn off **Run Dream consolidation on push**, or use an `Admin` key.

## Edits right after opening Obsidian do not sync

AMC starts watching for edits after Obsidian finishes loading, so the initial load does not flood the server.
Edit the note again, or run **Sync vault with Aether**.

## The agent cannot find a note

- Check the note's frontmatter. `aether_tier: private`, `.aetherignore`, and the built-in exclusions all hide notes from agents.
- With a scoped key, the agent sees only notes in its scopes plus `global`.
- Run **Sync vault with Aether**, then check the notice for updated and indexed counts.
- Run `aether-mcp doctor --vault /path/to/vault` to check the vault and client configuration.

## A proposed memory never appears in the inbox

Run **Sync vault with Aether**, or set **Auto-pull interval (seconds)** to a positive number.
The inbox pane shows what the server has staged.
