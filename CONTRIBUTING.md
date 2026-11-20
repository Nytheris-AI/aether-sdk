# Contributing to Aether SDK

Thank you for considering contributing to **Aether SDK**! Please also read [AGENTS.md](AGENTS.md) for commit and TDD conventions.

## Development Setup

Install a stable Rust toolchain with `rustfmt` and `clippy`:

```bash
git clone https://github.com/muninn-hq/aether-sdk.git
cd aether-sdk
cargo test --workspace --all-features
```

## Running Checks

Before opening a pull request, verify the full workspace gate passes:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

## Documentation

Documentation lives in `docs/` and is built with Docusaurus from `website/`:

```bash
cd website
npm ci
npm run start
```

## Pull Requests

1. Create a focused branch from `main`.
2. Follow TDD (Red - Green - Refactor) and keep commits small and atomic.
3. Ensure formatting, clippy (`-D warnings`), and all workspace tests pass locally.
4. Open a pull request with a clear description of the change and verification output.
