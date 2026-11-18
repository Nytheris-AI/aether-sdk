# Usage Guide

Aether SDK can be entered at any of five levels depending on how much of the pipeline you want to own. Every example below is runnable verbatim with `cargo run -p nytheris-aether-sdk --example <name>`.

---

## Level 1: Direct Connector (`level_1_connector.rs`)

Use a single `RecordStore`, `VectorStore`, or `GraphStore` directly without middleware, indexers, or `MemoryManager`:

```rust
use nytheris_aether_adapters::InMemoryBackend;
use nytheris_aether_core::{DistanceMetric, MemoryId, ProjectId, VectorRecord, VectorStore};
use serde_json::json;

#[tokio::main]
async fn main() -> nytheris_aether_core::Result<()> {
    let store = InMemoryBackend::new();
    let ns = ProjectId::new("level1-demo")?;
    store.initialize(&ns, 3, DistanceMetric::Cosine).await?;

    let id = MemoryId::new();
    store
        .upsert_batch(
            &ns,
            &[VectorRecord {
                id,
                vector: vec![1.0, 0.0, 0.0],
                metadata: json!({"topic": "connectors"}),
            }],
        )
        .await?;

    let hits = store.search(&ns, &[1.0, 0.0, 0.0], 5, None).await?;
    println!("level 1 hits: {} (top id={})", hits.len(), hits[0].id);
    Ok(())
}
```

---

## Level 2: Middleware Decorators (`level_2_middleware.rs`)

Wrap any connector in `Batcher`, `Timeout`, `Resilient`, and `Traced`:

```rust
use std::sync::Arc;
use std::time::Duration;
use nytheris_aether_adapters::{Batcher, FaultPoint, InMemoryBackend, Resilient, RetryPolicy, Timeout};
use nytheris_aether_core::{DistanceMetric, MemoryId, ProjectId, VectorRecord, VectorStore};
use serde_json::json;

#[tokio::main]
async fn main() -> nytheris_aether_core::Result<()> {
    let raw = Arc::new(InMemoryBackend::new());
    let ns = ProjectId::new("level2-demo")?;
    raw.initialize(&ns, 2, DistanceMetric::Cosine).await?;

    let store = Resilient::new(
        Timeout::new(Batcher::new(raw.clone()), Duration::from_secs(2)),
        RetryPolicy {
            max_retries: 3,
            base_delay: Duration::from_millis(5),
        },
    );

    raw.fail_next(FaultPoint::VectorUpsert, 1);
    store
        .upsert_batch(
            &ns,
            &[VectorRecord {
                id: MemoryId::new(),
                vector: vec![1.0, 0.0],
                metadata: json!({"retry": true}),
            }],
        )
        .await?;

    let hits = store.search(&ns, &[1.0, 0.0], 5, None).await?;
    println!("level 2 hits after retry: {}", hits.len());
    Ok(())
}
```

---

## Level 3: Composable Hybrid Recall (`level_3_hybrid_recall.rs`)

Call `nytheris_aether_cortex::recall::{vector_search, graph_search, rrf, RRF_K}` and `PersonalizedPageRank` directly to fuse dense vector search with graph traversal:

```rust
let vec_cands =
    recall::vector_search(&backend, &backend, &ns, &[1.0, 0.0], 5, None).await?;
let (graph_cands, subgraph) = recall::graph_search(
    &backend,
    &backend,
    &ns,
    &[alice.id],
    2,
    10,
    &TemporalFilter::default(),
    &PersonalizedPageRank::default(),
)
.await?;

let fused = recall::rrf(&[vec_cands, graph_cands], RRF_K);
```

---

## Level 4: Durable Indexer Sync Engine (`level_4_sync_engine.rs`)

Write authoritative records to a `RecordStore`, then drive derived vector and graph updates explicitly via `Indexer::run_pass()` (or opt into an interval loop with `Indexer::spawn`):

```rust
let indexer = Indexer::new(backend.clone())
    .with_vector(backend.clone())
    .with_graph(backend.clone());
let report = indexer.run_pass().await?;
```

---

## Level 5: `MemoryManager` Facade (`level_5_facade.rs`)

Assemble an authoritative `RecordStore` with optional vector and graph indexes using `MemoryManager::builder`:

```rust
use std::sync::Arc;
use nytheris_aether_adapters::InMemoryBackend;
use nytheris_aether_core::{
    DistanceMetric, Entity, EntityId, MemoryInput, MemoryUpdate, ProjectId, RecallMode,
    RecallQuery, Relation, VectorStore, WriteDurability,
};
use nytheris_aether_sdk::MemoryManager;
use chrono::Utc;

#[tokio::main]
async fn main() -> nytheris_aether_core::Result<()> {
    let backend = Arc::new(InMemoryBackend::new());
    let ns = ProjectId::new("level5-demo")?;
    backend.initialize(&ns, 2, DistanceMetric::Cosine).await?;

    let manager = MemoryManager::builder(backend.clone())
        .vector(backend.clone())
        .graph(backend)
        .build();

    let alice = Entity {
        id: EntityId::new("person:alice")?,
        name: "Alice".into(),
        entity_type: "Person".into(),
    };
    let mut input = MemoryInput::new(ns.clone(), "Alice prefers Qdrant");
    input.embedding = Some(vec![1.0, 0.0]);
    input.durability = WriteDurability::Buffered;
    input.entities = vec![alice.clone()];
    input.relations = vec![Relation {
        source: alice.id.clone(),
        target: EntityId::new("db:qdrant")?,
        relation_type: "PREFERS".into(),
        valid_at: Utc::now(),
        invalid_at: None,
    }];
    let receipt = manager.remember(input).await?;
    manager.flush().await?;

    let mut q = RecallQuery::new(ns.clone(), "What database does Alice prefer?");
    q.query_vector = Some(vec![1.0, 0.0]);
    q.seed_entities = vec![alice.id];
    q.mode = RecallMode::Hybrid;
    let res = manager.recall(q).await?;
    println!("level 5 recalled hits: {}, edges: {}", res.hits.len(), res.subgraph.edges.len());

    let updated = manager
        .update(
            &ns,
            receipt.memory_id,
            MemoryUpdate {
                content: Some("Alice now prefers pgvector".into()),
                embedding: Some(vec![0.0, 1.0]),
                metadata: None,
                entities: None,
                relations: None,
                expected_version: Some(receipt.version),
            },
        )
        .await?;
    println!("level 5 updated version: {}", updated.version);

    manager.forget(&ns, receipt.memory_id).await?;
    manager.shutdown().await?;
    Ok(())
}
```
