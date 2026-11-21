//! Level 5: Full memory lifecycle through the optional `MemoryManager` facade.

use std::sync::Arc;

use chrono::Utc;
use muninn_aether_adapters::InMemoryBackend;
use muninn_aether_core::{
    DistanceMetric, Entity, EntityId, MemoryInput, MemoryUpdate, ProjectId, RecallMode,
    RecallQuery, Relation, VectorStore, WriteDurability,
};
use muninn_aether_sdk::MemoryManager;

#[tokio::main]
async fn main() -> muninn_aether_core::Result<()> {
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
    println!(
        "level 5 recalled hits: {}, edges: {}",
        res.hits.len(),
        res.subgraph.edges.len()
    );

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
