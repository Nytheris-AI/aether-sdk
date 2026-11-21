---
sidebar_position: 2
---

# Install aether-mcp

`aether-mcp` is the local server that indexes your vault and answers agent requests.
Prebuilt binaries are published on GitHub Releases, so you do not need Rust or Node.js.

## Supported platforms

| Platform | Architectures |
| --- | --- |
| Linux | x86_64, arm64 |
| macOS | Apple Silicon, Intel |
| Windows | x86_64 |

## Option A: installer and guided setup (recommended for desktop)

Install the latest release binary:

```bash
# macOS / Linux
curl -fsSL https://raw.githubusercontent.com/muninn-hq/aether-sdk/main/scripts/install.sh | sh
```

```powershell
# Windows (PowerShell)
irm https://raw.githubusercontent.com/muninn-hq/aether-sdk/main/scripts/install.ps1 | iex
```

When the install finishes, the installer asks whether to run the guided setup.
Answer yes, or run it later:

```bash
aether-mcp setup
```

`setup` does the following, and asks before each change:

1. Finds the vaults Obsidian knows about and asks which one to use. You can also choose the current folder, browse for a folder, or type a path.
2. Installs the background service for that vault (`launchd` on macOS, `systemd --user` on Linux, Task Scheduler on Windows) and checks that the server answers.
3. Shows a checklist of the MCP clients it finds. Nothing is checked by default, so check the clients you want, or leave them all unchecked and add `aether-vault` to your IDE yourself. Checked clients keep their other servers.
4. Runs the same checks as `aether-mcp doctor` and prints what to do next.

Running it again is safe, and clients that are already configured are left alone.

To remove everything again, run `aether-mcp uninstall`. It stops the background service and removes `aether-vault` from the MCP clients, keeping their other servers. Add `--vault <path>` to also remove that vault's AMC plugin files, and `--purge-vault` to delete its index and `Aether/` folder. Your notes are never deleted. The command asks before it changes anything, or pass `--yes` to skip the question.

| Flag | Effect |
| --- | --- |
| `--vault <path>` | Use this vault instead of asking |
| `--yes` | Accept the defaults without asking. Required when there is no terminal, for example in a script |
| `--no-clients` | Do not change MCP client configuration (the checklist is not shown) |
| `--no-service` | Do not install the background service |

Set `AETHER_NO_SETUP=1` before running the installer to skip the offer.

### Without the guided setup

Register `aether-mcp` as an auto-starting background service yourself:

```bash
aether-mcp service install --vault /path/to/vault
```

Check or remove the service at any time:

```bash
aether-mcp service status
aether-mcp service uninstall
```

Or run the server in the foreground instead:

```bash
aether-mcp --vault /path/to/vault --watch-ms 2000 --http-addr 127.0.0.1:14771 --http-only
```

`--http-only` keeps the HTTP server alive until `Ctrl-C` or `SIGTERM` without waiting on `stdin`.
The server stores its index at `<vault>/.aether/aether.sqlite` and checks the vault for changes every `2000` milliseconds.
Without `--require-auth`, it accepts connections only from `127.0.0.1` and `::1`.

## Option B: npm (Node.js 18 or later)

Install the npm package globally, which puts `aether-mcp` on your PATH:

```bash
npm install -g @muninn-org/aether-mcp
aether-mcp setup
```

On first use the launcher downloads the matching release binary from GitHub Releases, checks its SHA-256 checksum, and caches it.
Later runs start immediately, and every argument is passed to `aether-mcp` unchanged.
It needs `tar` on your PATH, which macOS, Linux, and Windows 10 and later include.

If you do not want a global install, `npx @muninn-org/aether-mcp setup` runs the same launcher without installing it.

An MCP client can also start the server through `npx`:

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

| Environment variable | Purpose |
| --- | --- |
| `AETHER_MCP_VERSION` | Use a different `aether-mcp` release than the one the package was tested with |
| `AETHER_MCP_CACHE_DIR` | Where downloaded binaries are kept |
| `AETHER_MCP_DOWNLOAD_BASE` | Download releases from a mirror instead of GitHub |

## Option C: Docker (recommended for team or remote gateways)

Run the multi-arch image from GitHub Container Registry with a mounted vault and bearer authentication:

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
Without it, `aether-mcp` also starts its stdio loop, which ends immediately in a detached container and stops the server.
`--user` makes the container write to your vault as you, so files keep your ownership and permissions.

## Option D: build from source (contributors)

In the `aether-sdk` repository:

```bash
cargo build --release -p muninn-aether-vault --bin aether-mcp
```

## Verify the server

```bash
aether-mcp --version
aether-mcp doctor --vault /path/to/vault
```

`doctor` checks the vault, the server binary, and your detected MCP client configs.
In a terminal it shows a status mark for each check, and it prints a hint next to anything that needs fixing.
When the output is piped, or `NO_COLOR` is set, it prints plain text.

To get tab completion, generate a script for your shell, for example `aether-mcp completions bash`.
It supports `bash`, `zsh`, `fish`, `powershell`, and `elvish`.

## Next step

[Install AMC in Obsidian](./install-amc.md).
