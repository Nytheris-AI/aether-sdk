# Usage Guide

This guide demonstrates how to initialize `MemoryManager` and perform the full memory lifecycle (`remember`, `recall`, `update`, `forget`, `flush`, `shutdown`) using the zero-infra `InMemoryBackend`.

## Quickstart (Rust)

```rust
use std::sync::Arc;
use aether_adapters::InMemoryBackend;
use aether_core::{
    DistanceMetric, Entity, EntityId, MemoryInbound, MemoryInput, MemoryUpdate,
    ProjectId, RecallMode, RecallQuery, Relation, VectorStore, WriteDurability,
};
use aether_sdk::MemoryManager;
use chrono::Utc;

#[tokio::main]
async fn main() -> aether_core::Result<()> {
    let backend = Arc::new(InMemoryBackend::new());
    let project_id = ProjectId::new("proj-alpha")?;

    // Initialize vector index dimensions & distance metric
    backend.initialize(&project_id, 3, DistanceMetric::Cosine).await?;

    // Construct the unified L1/L2/L3 MemoryManager facade
    let manager: Arc<dyn MemoryInbound> = Arc::new(MemoryManager::new(
        backend.clone(), // L2 KvStore
        backend.clone(), // L3 VectorStore
        backend.clone(), // L3 GraphStore
    ));

    // 1. Remember (with optional pre-computed embedding, entities, and relations)
    let mut input = MemoryInput::new(project_id.clone(), "Alice uses Turbopuffer for vector recall.");
    input.embedding = Some(vec![1.0, 0.0, 0.0]);
    input.durability = WriteDurability::WaitFlush;
    input.entities = vec![Entity {
        id: EntityId::new("person:alice")?,
        name: "Alice".into(),
        entity_type: "Person".into(),
    }];
    input.relations = vec![Relation {
        source: EntityId::new("person:alice")?,
        target: EntityId::new("db:turbopuffer")?,
        relation_type: "USES".into(),
        valid_at: Utc::now(),
        invalid_at: None,
    }];

    let receipt = manager.remember(input).await?;

    // 2. Hybrid Recall (Vector + Graph neighborhood + ReadYourWrites)
    let mut query = RecallQuery::new(project_id.clone(), "Alice vector database");
    query.query_vector = Some(vec![1.0, 0.0, 0.0]);
    query.seed_entities = vec![EntityId::new("person:alice")?];
    query.mode = RecallMode::Hybrid;

    let result = manager.recall(query).await?;
    println!("Hits: {}, Graph edges: {}", result.hits.len(), result.subgraph.edges.len());

    // 3. Optimistic Concurrency (CAS) Update
    manager.update(
        &project_id,
        receipt.memory_id,
        MemoryUpdate {
            content: Some("Alice uses Turbopuffer and Neo4j.".into()),
            embedding: None,
            metadata: None,
            expected_version: Some(receipt.version),
        },
    ).await?;

    // 4. Forget (deletes vector + retracts graph provenance edges)
    manager.forget(&project_id, receipt.memory_id).await?;

    // 5. Graceful shutdown
    manager.shutdown().await?;
    Ok(())
}
```
