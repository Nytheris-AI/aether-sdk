---
sidebar_position: 2
---

# Install aether-mcp

`aether-mcp` is the local server that indexes your vault and answers agent requests.
Prebuilt binaries are published on GitHub Releases, so you do not need Rust or Node.js.

## Option A: installer and background service (recommended for desktop)

Install the latest release binary:

```bash
# macOS / Linux
curl -fsSL https://raw.githubusercontent.com/muninn-hq/aether-sdk/main/scripts/install.sh | sh
```

```powershell
# Windows (PowerShell)
irm https://raw.githubusercontent.com/muninn-hq/aether-sdk/main/scripts/install.ps1 | iex
```

Register `aether-mcp` as an auto-starting background service for your vault.
It uses `launchd` on macOS, `systemd --user` on Linux, and Task Scheduler on Windows.

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

## Option B: Docker (recommended for team or remote gateways)

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

## Option C: build from source (contributors)

In the `aether-sdk` repository:

```bash
cargo build --release -p muninn-aether-vault --bin aether-mcp
```

## Verify the server

```bash
aether-mcp doctor --vault /path/to/vault
```

The command checks the vault and your detected MCP client configuration.

## Next step

[Install AMC in Obsidian](./install-amc.md).
