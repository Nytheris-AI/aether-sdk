# Development Guidelines

Engineering conventions for working on the **Aether SDK** Rust workspace. See [`AGENTS.md`](https://github.com/muninn-hq/aether-sdk/blob/main/AGENTS.md) for repository-wide micro-commit and TDD rules.

## Workspace Layout

- `crates/aether-core` - Domain DTOs, validated newtypes (`ProjectId`, `MemoryId`, `EntityId`, `UserId`, `SessionId`), `DistanceMetric::score`, `FilterExpr::matches`, `IndexSpec`, `AetherError`, and storage traits (`OutboundConnector`, `RecordStore`, `VectorStore`, `GraphStore`).
- `crates/aether-cortex` - Composable sync and retrieval primitives (`Indexer`, `PersonalizedPageRank`, `graph_scores`, `recall::{scope_filter, vector_search, unindexed, session, graph_search, merge, rrf, in_scopes}`).
- `crates/aether-adapters` - Storage connectors (`sqlite`, `redis`, `turbopuffer`, `qdrant`, `chroma`, `pgvector`, `neo4j`), resilience middleware (`Batcher`, `Timeout`, `Resilient`, `Traced`), `InMemoryBackend` (`in_memory` module with `FaultPoint`), and reusable `contract` test suite (`contract-suite` feature).
- `crates/aether-sdk` - Optional `MemoryManager` / `MemoryManagerBuilder` facade, runnable examples (`level_1_connector` .. `level_5_facade`), and optional `telemetry` (`otel` feature).
- `crates/aether-vault` - Obsidian-native vault daemon (`VaultEngine`), single-file SQLite WAL + FTS5 + 256-dim local vector + `PersonalizedPageRank` hybrid recall, `Aether/Inbox.md` staging and reconciliation, and 3-tool stdio MCP server (`McpServer`, `aether-mcp` binary).

## Core Principles

1. **Model-free boundary** - Aether does not call LLMs, embedders, chunkers, or extractors. Callers supply text, optional vectors, entities, and relations.
2. **Pieces-first isolation** - Write orchestration against `RecordStore`, `VectorStore`, and `GraphStore` traits. Offline tests run deterministically against `InMemoryBackend` without external services; live database tests are `#[ignore]`d and opt-in.
3. **Explicit error classification** - Classify retryable transport/rate-limit faults as `AetherError::Transient` and invalid input/auth/schema/version conflicts as permanent variants (`Permanent`, `InvalidInput`, `NotFound`, `Conflict`) so `Resilient` only retries transient faults.
4. **Shared backend contract** - Every `RecordStore`, `VectorStore`, and `GraphStore` driver must pass `muninn_aether_adapters::contract::{record_store, vector_store, graph_store}` enabled via the `contract-suite` feature.

## Verification Gate

Every change must pass:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```
