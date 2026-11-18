# Aether SDK

**Model-free Rust memory SDK** for storing, indexing, and retrieving AI memory across durable **record, vector, and graph stores** (**SQLite, Redis/Valkey, Turbopuffer, Qdrant, Chroma, pgvector, and Neo4j**).

Aether exposes five composable levels across four workspace crates so you can adopt individual connectors, resilience middleware, recall primitives, the `Indexer` sync engine, or the optional `MemoryManager` facade:

- **`aether-core`** - Domain types, validation newtypes (`ProjectId`, `MemoryId`, `EntityId`, `UserId`, `SessionId`), `IndexSpec`, `AetherError` (`Transient` vs `Permanent`), and storage traits (`RecordStore`, `VectorStore`, `GraphStore`).
- **`aether-cortex`** - `Indexer` sync engine, `PersonalizedPageRank` + `graph_scores`, and composable `recall` steps (`vector_search`, `unindexed`, `session`, `graph_search`, `merge`, `rrf`).
- **`aether-adapters`** - `InMemoryBackend` (`in_memory`, with `FaultPoint` injection), middleware (`Batcher`, `Timeout`, `Resilient` with full-jitter backoff, `Traced`), reusable `contract` test suite (`contract-suite` feature), and 7 storage drivers (`sqlite`, `redis`, `turbopuffer`, `qdrant`, `chroma`, `pgvector`, `neo4j`).
- **`aether-sdk`** - Public `MemoryManager` / `MemoryManagerBuilder` facade and optional OpenTelemetry preset (`otel` feature).

## Quickstart

Run the full offline workspace test suite:

```bash
cargo test --workspace --all-features
```

Run the per-level examples (`crates/aether-sdk/examples/`):

```bash
cargo run -p aether-sdk --example level_1_connector
cargo run -p aether-sdk --example level_2_middleware
cargo run -p aether-sdk --example level_3_hybrid_recall
cargo run -p aether-sdk --example level_4_sync_engine
cargo run -p aether-sdk --example level_5_facade
cargo run -p aether-sdk --example otel_demo --features otel
```

Use `MemoryManager` with `InMemoryBackend` in Rust:

```rust
use std::sync::Arc;
use aether_adapters::InMemoryBackend;
use aether_core::{DistanceMetric, MemoryInput, ProjectId, RecallMode, RecallQuery, VectorStore};
use aether_sdk::MemoryManager;

#[tokio::main]
async fn main() -> aether_core::Result<()> {
    let backend = Arc::new(InMemoryBackend::new());
    let project_id = ProjectId::new("my-project")?;
    backend.initialize(&project_id, 3, DistanceMetric::Cosine).await?;

    let manager = MemoryManager::builder(backend.clone())
        .vector(backend.clone())
        .graph(backend)
        .build();

    let mut input = MemoryInput::new(project_id.clone(), "Aether SDK keeps stores consistent.");
    input.embedding = Some(vec![1.0, 0.0, 0.0]);
    manager.remember(input).await?;
    manager.flush().await?;

    let mut query = RecallQuery::new(project_id, "consistent stores");
    query.query_vector = Some(vec![1.0, 0.0, 0.0]);
    query.mode = RecallMode::Vector;
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
