# aether-mcp guided setup and CLI polish

Status: draft for approval.

## Intent

A new user should get from "I want my agent to recall my Obsidian notes" to a working `aether-mcp` with as few steps and as little guessing as possible.
Where setup needs extra input, the tool asks for it and explains the next step in plain language.
The CLI should also look polished, because it is the first thing a new user sees.

Today the path is about six manual steps: run the installer, run `service install`, install the plugin, enable it, paste an `mcp.json` snippet, and check the connection.
The plugin step (BRAT during the beta) is a known and accepted friction and is out of scope.

## Goals

- One guided command, `aether-mcp setup`, that finds the user's Obsidian vaults, installs the background service, writes MCP client configuration, verifies the result, and prints the remaining steps.
- More ways to get the binary: a Linux arm64 build and an `npx` entry point.
- A polished terminal experience: styled prompts, spinners, step lists, and clear errors.
- `--help` and shell completions that come from one definition of the CLI.

## Non-goals

- Installing or enabling the Obsidian plugin. BRAT stays the install path until the directory listing is approved.
- A full-screen terminal UI. A live `status --watch` dashboard may come later and is not part of this work.
- Homebrew, winget, or scoop packaging. Each needs its own release automation, so they come later if there is demand.
- Changing server behavior, the HTTP API, or the MCP tools.

## Anti-goals

- Breaking any existing flag, subcommand, or output that scripts, the systemd unit, the Docker image, or the installers rely on.
- Prompts or color appearing when output is piped or when `NO_COLOR` is set.
- Setup silently overwriting a user's existing MCP client configuration.

## Constraints

- Workspace MSRV is Rust 1.89, edition 2021.
- Dependency licenses must pass `deny.toml`. `clap` is MIT OR Apache-2.0 and `cliclack` is MIT, both allowed.
- The `--init` command prints an `mcp.json` snippet on stdout, and `service install` is run by users and scripts. Their output formats stay stable.
- Windows, macOS, and Linux must all work. `cliclack` has not been verified on Windows yet.
- The existing tests in `crates/aether-vault/src/bin/aether-mcp.rs` are the safety net for the parser change.

## Approaches considered

**A. One guided `setup` command only.**
Biggest friction cut and one code path for every entry point.
Does not help people who find the tool through `npx`.

**B. More distribution channels only (`npx`, Homebrew, winget, arm64).**
Reaches users where they are, but each channel still leaves the manual steps in place.

**C. A guided `setup` command plus `npx` and a Linux arm64 build (chosen).**
`setup` is the single flow, and `npx` is a thin launcher for it.
Homebrew and winget wait until there is demand.

For the terminal toolkit:

| Option | Verdict |
| --- | --- |
| `clap` for parsing, `cliclack` for prompts and progress (chosen) | Linear output that stays in the scrollback, falls back cleanly when not a terminal, small surface. |
| `ratatui` | A full-screen framework that redraws the whole terminal. Wrong shape for a wizard, and `--help`, `service`, Docker, CI, and piped installs would need a second plain-text path. |
| `dialoguer` and `indicatif` | Works, but `cliclack` gives a more consistent look with one dependency. |

## Design

### Parsing: `clap`

Replace the hand-written parser with `clap` derive definitions.
Every existing flag, environment variable, and subcommand keeps its name and meaning (see `docs/reference/aether-mcp.md`).
This gives proper `--help`, error messages with suggestions, and `aether-mcp completions <shell>`.
This is a refactor, done in its own commit with the existing parser tests passing unchanged.

### Output: one styling layer

A small module decides once, at startup, whether output is interactive: stdout is a terminal and `NO_COLOR` is not set.
Interactive output uses `cliclack` styling.
Non-interactive output is the same text without styling or prompts.
`doctor`, `service`, `--init`, and `setup` all go through it.
Machine-readable output, such as the `mcp.json` snippet on stdout, is never styled.

### `aether-mcp setup`

Flow:

1. **Find vaults.** Read Obsidian's own `obsidian.json` (standard location per OS, plus the Flatpak path on Linux) and offer the vaults it lists. Allow typing a path. With one vault found, confirm it instead of asking.
2. **Background service.** Install and start the user service for the chosen vault, using the existing `service install` logic.
3. **MCP clients.** Detect installed clients (Cursor, Claude Desktop, Claude Code, Antigravity) and show which ones will be configured. Ask for confirmation. Reuse the existing client-config writer, which merges into the file and never replaces other servers.
4. **Verify.** Run the `doctor` checks and show a pass or fail line for each.
5. **Next steps.** Print what is left: install the AMC plugin through BRAT (with the repository URL), then enable it, then ask the agent a question. Link to the docs quickstart.

Flags make it scriptable: `--vault <path>`, `--yes` to accept defaults, and `--no-clients` to skip client configuration.
With stdin not a terminal and no `--yes`, `setup` prints what it would do and exits with a clear message instead of hanging on a prompt.
Running it twice is safe: each step checks current state and reports "already set up".

### `doctor`

Keep the current checks and add a fix hint to each failure, for example "service is not running: run `aether-mcp service install --vault <path>`".
Do not auto-fix. `setup` is the guided fixer.

### Distribution

- **Linux arm64.** Add `aarch64-unknown-linux-gnu` to the release matrix and to `scripts/install.sh`. The Docker image is already multi-arch.
- **`npx` entry point.** A small npm package that, on first run, downloads the matching release binary, verifies its SHA-256 checksum the same way `install.sh` does, caches it, and runs it with the user's arguments. `npx <package> setup` is the headline command. MCP clients can also use it as `"command": "npx"`.
- **Installer scripts.** After installing, they offer to run `aether-mcp setup` when attached to a terminal, and print the command otherwise.

## Order of work

Each step is its own micro commit or small group of commits, and each builds and passes tests alone.

1. Add `clap` and port the parser, keeping all current tests green.
2. Add the output module and restyle `doctor` and `service`.
3. Add `setup`, with tests for vault discovery, idempotence, and the non-interactive path.
4. Add the Linux arm64 build and installer support.
5. Add the `npx` package and its release workflow.
6. Update the docs: quickstart, install pages, CLI reference.

## Production impact

- Existing flags, subcommands, environment variables, and the `mcp.json` stdout format are unchanged, so installed services and scripts keep working.
- The release gains one more artifact, and the npm package adds a new publish step to maintain.
- Binary size grows slightly from the two new dependencies.

## Decisions left to the builder

- Exact `cliclack` wording and layout. The tone is plain and short, with no jargon.
- The npm package name and scope. This needs an owner on npm, so it is an open question for the maintainers.
- Whether the Windows terminal needs a fallback for unicode symbols.

## Open items to verify before step 2

- `cliclack` builds on Windows and macOS and passes `cargo deny`.
- The `clap` build keeps `aether-mcp --help` output short enough to read.
