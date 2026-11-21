# aether-mcp clap parser Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the hand-written argument loop in `aether-mcp` with `clap`, keeping every existing flag, env fallback, and mode check, and add `--help`, `--version`, and shell completions.

**Architecture:** A new `cli.rs` module next to the binary holds the `clap` derive structs and returns the raw parsed values. `parse_cli_args_with_env` in `aether-mcp.rs` keeps its signature, env fallbacks, mode selection, and `AetherError::InvalidInput` errors, and reads the raw values instead of looping over `args`.

**Tech Stack:** Rust 1.89, `clap` 4 (derive), `clap_complete`.

**Spec:** `docs/superpowers/specs/2026-11-21-aether-mcp-setup-and-cli-design.md` (step 1 of "Order of work").

## Global Constraints

- Every existing flag, environment variable, and subcommand keeps its name and meaning (`docs/reference/aether-mcp.md`).
- Existing tests in `crates/aether-vault/src/bin/aether-mcp.rs` pass unchanged.
- Parse errors stay `AetherError::InvalidInput`.
- Licenses must pass `deny.toml` (`clap` and `clap_complete` are MIT OR Apache-2.0).
- `cargo clippy` and `cargo fmt --check` stay clean. MSRV 1.89.

## Review Focus

- `--help` and `--version` print and exit 0 instead of reaching the server.
- Options after a subcommand still work (`service install --vault x`, `doctor --vault x`).
- `doctor --init` and `service ... --http-only` still fail with `InvalidInput`.
- Unknown flags and a non-numeric `--watch-ms` now fail with a clear error (previously ignored). Intentional.
- Env fallbacks still apply when a flag is absent and lose when the flag is present.

---

### Task 1: Port the parser to clap

**Files:**
- Modify: `crates/aether-vault/Cargo.toml` (add `clap = { version = "4", features = ["derive"] }`)
- Create: `crates/aether-vault/src/bin/aether-mcp/cli.rs`
- Modify: `crates/aether-vault/src/bin/aether-mcp.rs` (`mod cli;`, `parse_cli_args_with_env`)

**Interfaces:**
- Produces: `cli::RawCli::try_parse_from(&[String]) -> std::result::Result<RawCli, clap::Error>` with `Option` fields `vault: Option<PathBuf>`, `db: Option<PathBuf>`, `watch_ms: Option<u64>`, `http_addr: Option<String>`, `bootstrap_admin_key`, `api_key`, `kv_endpoint`, `kv_model: Option<String>`, bool flags `http_only`, `init`, `install_clients`, `require_auth`, and `command: Option<Command>` where `Command` is `Service { action: ServiceCmd }` or `Doctor`.

- [ ] **Step 1: Write failing tests** in the `tests` module of `aether-mcp.rs`:
  - `help_flag_is_not_an_invalid_input_error`: `parse_cli_args(&args(&["aether-mcp", "--help"]))` returns an error whose message contains `Usage`.
  - `unknown_flag_returns_invalid_input`: `--bogus` returns `InvalidInput`.
  - `non_numeric_watch_ms_returns_invalid_input`: `--watch-ms abc` returns `InvalidInput`.
  - `flag_overrides_env_for_http_addr`: `--http-addr 127.0.0.1:1` wins over `AETHER_HTTP_ADDR=127.0.0.1:2`.
- [ ] **Step 2: Run** `cargo test -p muninn-aether-vault --bin aether-mcp`. Expected: the first three FAIL.
- [ ] **Step 3: Implement** `cli.rs` with `#[derive(Parser)]`, `global = true` on every option so they work after a subcommand, `disable_help_subcommand`, version from `CARGO_PKG_VERSION`. Replace the `while idx` loop and the `service`/`doctor` prefix detection in `parse_cli_args_with_env` with `RawCli::try_parse_from`. Map `clap::Error` to `AetherError::InvalidInput(err.to_string())`. Keep the env fallbacks, `http_addr` default for `service install`, and the mode match unchanged.
- [ ] **Step 4: Run** `cargo test -p muninn-aether-vault --bin aether-mcp`. Expected: all pass, including the pre-existing tests.
- [ ] **Step 5: Commit** `refactor: parse aether-mcp arguments with clap`.

### Task 2: Print help and version, add completions

**Files:**
- Modify: `crates/aether-vault/Cargo.toml` (add `clap_complete = "4"`), `cli.rs`, `aether-mcp.rs`

**Interfaces:**
- Consumes: `RawCli` from Task 1.
- Produces: `aether-mcp completions <bash|zsh|fish|powershell|elvish>` writing the script to stdout. `--help` and `--version` exit 0 through `clap`'s own printing in `main`.

- [ ] **Step 1: Write failing test** `completions_script_mentions_service_subcommand`: `cli::completions_script(Shell::Bash)` returns a string containing `service`.
- [ ] **Step 2: Run** the test. Expected: FAIL (function missing).
- [ ] **Step 3: Implement** `completions_script(shell) -> String` in `cli.rs` using `clap_complete::generate`, a `Completions { shell }` subcommand, and in `main` handle `DisplayHelp`/`DisplayVersion` by printing and exiting 0 before the parser maps errors.
- [ ] **Step 4: Run** the full crate tests, `cargo clippy -p muninn-aether-vault --all-targets -- -D warnings`, `cargo fmt --check`, and `cargo run -p muninn-aether-vault --bin aether-mcp -- --help`. Expected: all clean, help lists every flag.
- [ ] **Step 5: Commit** `feat: add aether-mcp completions and --version`.
