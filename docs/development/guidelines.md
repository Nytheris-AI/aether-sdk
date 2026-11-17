# Development Guidelines

Engineering conventions for working on the **Aether SDK** Rust workspace. See [`AGENTS.md`](https://github.com/Nytheris-AI/aether-sdk/blob/main/AGENTS.md) for repository-wide micro-commit and TDD rules.

## Workspace Layout

- `crates/aether-core` - Domain DTOs, validated newtypes (`ProjectId`, `MemoryId`, `EntityId`), `AetherError`, and outbound/inbound traits (`RecordStore`, `VectorStore`, `GraphStore`, `OutboundConnector`, `MemoryInbound`).
- `crates/aether-cortex` - Pure coordination and retrieval logic (`PolicyRouter`, `HotContextCache`, `IngestionBuffer`, `QueryPlanner`, `personalized_page_rank`).
- `crates/aether-adapters` - Storage connectors, resilience middleware (`Traced`, `Resilient`, `Timeout`, `Batcher`), `InMemoryBackend` (with `FaultPoint` injection), and the reusable `contract` module (`contract-suite` feature).
- `crates/aether-sdk` - Public `MemoryManager` facade and optional `telemetry` (`otel` feature).

## Core Principles

1. **Model-free boundary** - Aether does not call LLMs, embedders, chunkers, or extractors. Callers supply text, optional vectors, entities, and relations.
2. **Trait-first isolation** - Code against `RecordStore`, `VectorStore`, and `GraphStore` rather than concrete backends. Unit and integration tests run against `InMemoryBackend` without external services.
3. **Explicit error classification** - Distinguish retryable failures (`AetherError::Transient`) from non-retryable failures (`AetherError::Permanent`, `InvalidInput`, `NotFound`, `Conflict`) so `Resilient` only retries transient faults.
4. **Shared backend contract** - Every `RecordStore`, `VectorStore`, and `GraphStore` implementation must pass `aether_adapters::contract::{record_store, vector_store, graph_store}` enabled via the `contract-suite` feature.

## Verification Gate

Every commit must pass:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```
