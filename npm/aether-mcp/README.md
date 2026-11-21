# @muninn-org/aether-mcp

Run [aether-mcp](https://github.com/muninn-hq/aether-sdk), the local memory server for Obsidian and AI agents, without installing Rust or a separate binary.

```bash
npm install -g @muninn-org/aether-mcp
aether-mcp setup
```

To try it without a global install, run `npx @muninn-org/aether-mcp setup` instead.

On first use the launcher downloads the matching `aether-mcp` release for your machine from GitHub Releases, checks its SHA-256 checksum, and caches it. Later runs start immediately.
Every argument is passed straight through, so anything the `aether-mcp` CLI accepts works here.

Use it as an MCP server in a client configuration:

```json
{
  "mcpServers": {
    "aether-vault": {
      "command": "npx",
      "args": ["-y", "@muninn-org/aether-mcp", "--vault", "/path/to/vault", "--watch-ms", "0"]
    }
  }
}
```

Download progress goes to stderr, so stdout stays clean for the MCP protocol.

## Requirements

- Node.js 18 or later.
- `tar` on your PATH (included with macOS, Linux, and Windows 10 and later).
- Linux x64 or arm64, macOS x64 or arm64, or Windows x64.

## Settings

| Environment variable | Purpose |
| --- | --- |
| `AETHER_MCP_VERSION` | Use a different `aether-mcp` release than the one this package was tested with. |
| `AETHER_MCP_CACHE_DIR` | Where downloaded binaries are kept. |
| `AETHER_MCP_DOWNLOAD_BASE` | Download releases from a mirror instead of GitHub. |

## Documentation

[Quickstart](https://muninn-hq.github.io/aether-sdk/docs/get-started/quickstart) and the [CLI reference](https://muninn-hq.github.io/aether-sdk/docs/reference/aether-mcp).
