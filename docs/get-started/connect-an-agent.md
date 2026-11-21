---
sidebar_position: 4
---

# Connect an AI agent

Any MCP-compatible client can use your vault through `aether-mcp`, either over `stdio` or over HTTP.

## Connect over stdio

Add `aether-mcp` to your client's `mcpServers` configuration.
This works in Claude Desktop, Cursor, Claude Code, and Antigravity.

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

Pass `--watch-ms 0` when the background service from [Install aether-mcp](./install-aether-mcp.md) is already running.
Both processes share `<vault>/.aether/aether.sqlite` without running duplicate scans.
If the server uses `--require-auth`, add `"--api-key"` and your scoped `aek_...` token to `args`.

Run `aether-mcp --init --install-clients --vault /path/to/vault` to have the installer configure detected clients for you.

## Connect over HTTP or HTTPS

HTTP-capable agents can send JSON-RPC 2.0 requests to `POST http://127.0.0.1:14771/v1/mcp`.
REST tool clients such as ChatGPT Custom GPT Actions or Open WebUI can import `GET http://127.0.0.1:14771/v1/openapi.json` (OpenAPI `3.1.0`).
For an authenticated server, use `https://<host>/v1/openapi.json` with `Authorization: Bearer aek_...`.

## What your agent can do

Once connected, the agent has three tools:

| Tool | What it does |
| --- | --- |
| `aether_recall` | Returns your hot profile and the most relevant warm and cold notes within a token budget. |
| `aether_inspect_source` | Reads a verbatim Markdown section by pointer, for example `Arch/DB.md#Schema`. It refuses `private` notes and path traversal. |
| `aether_stage_memory` | Proposes a new fact. It waits in `Aether/Inbox.md` until you approve it. |

See [aether-mcp CLI and MCP tools](../reference/aether-mcp.md) for every argument.

## Check that it works

Ask your agent a question that one of your public notes answers.
Then ask about something that exists only in a `private` note.
The agent should find the first and get nothing for the second.

## Next steps

- [Organize and protect your notes](../guides/organize-and-protect-notes.md)
- [Review what your agent wants to remember](../guides/review-agent-memories.md)
