# Aether SDK

**Model-free Rust memory SDK** for storing, indexing, and retrieving AI memory across record, vector, and graph stores.

Aether provides composable building blocks and an optional `MemoryManager` facade across four workspace crates:

- **`aether-core`** - Domain types, validation newtypes (`ProjectId`, `MemoryId`, `EntityId`), `AetherError` (`Transient` vs `Permanent`), and storage traits (`RecordStore`, `VectorStore`, `GraphStore`, `MemoryInbound`).
- **`aether-cortex`** - L1 `HotContextCache`, `PolicyRouter`, `IngestionBuffer`, `QueryPlanner`, and Personalized PageRank (`personalized_page_rank`).
- **`aether-adapters`** - `InMemoryBackend` (with `FaultPoint` injection), resilience middleware (`Traced`, `Resilient`, `Timeout`, `Batcher`), and a reusable `contract` test suite (`contract-suite` feature).
- **`aether-sdk`** - Public `MemoryManager` / `MemoryManagerBuilder` facade and optional OpenTelemetry preset (`otel` feature).

## Quickstart

Run the full workspace test suite (uses zero-infra `InMemoryBackend`):

```bash
cargo test --workspace --all-features
```

Run the OpenTelemetry example (`crates/aether-sdk/examples/otel_demo.rs`):

```bash
cargo run -p aether-sdk --example otel_demo --features otel
```

Use `MemoryManager` with `InMemoryBackend` in Rust:

```rust
use std::sync::Arc;
use aether_adapters::InMemoryBackend;
use aether_core::{DistanceMetric, MemoryInbound, MemoryInput, ProjectId, RecallQuery, VectorStore};
use aether_sdk::MemoryManager;

#[tokio::main]
async fn main() -> aether_core::Result<()> {
    let backend = Arc::new(InMemoryBackend::new());
    let project_id = ProjectId::new("my-project")?;
    backend.initialize(&project_id, 3, DistanceMetric::Cosine).await?;

    let manager = MemoryManager::new(backend.clone(), backend.clone(), backend.clone());

    let mut input = MemoryInput::new(project_id.clone(), "Aether SDK keeps stores consistent.");
    input.embedding = Some(vec![1.0, 0.0, 0.0]);
    manager.remember(input).await?;

    let mut query = RecallQuery::new(project_id, "consistent stores");
    query.query_vector = Some(vec![1.0, 0.0, 0.0]);
    let result = manager.recall(query).await?;
    println!("Hits: {}", result.hits.len());
    Ok(())
}
```

## Development Gate

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

## Documentation

- Engineering rules: [AGENTS.md](AGENTS.md)
- Contributing guide: [CONTRIBUTING.md](CONTRIBUTING.md)
- Architecture & guides: [docs/index.md](docs/index.md)

## License

Apache 2.0 - see [LICENSE](LICENSE).
