//! Level 4: Use the durable `Indexer` sync engine directly to derive vector and graph indexes.

use std::sync::Arc;

use muninn_aether_adapters::InMemoryBackend;
use muninn_aether_core::{
    DistanceMetric, Entity, EntityId, MemoryInput, MemoryItem, ProjectId, RecordStore, VectorStore,
};
use muninn_aether_cortex::{recall, Indexer};

#[tokio::main]
async fn main() -> muninn_aether_core::Result<()> {
    let backend = Arc::new(InMemoryBackend::new());
    let ns = ProjectId::new("level4-demo")?;
    backend.initialize(&ns, 2, DistanceMetric::Cosine).await?;

    let mut input = MemoryInput::new(ns.clone(), "Alice owns payment-service");
    input.embedding = Some(vec![1.0, 0.0]);
    input.entities = vec![Entity {
        id: EntityId::new("person:alice")?,
        name: "Alice".into(),
        entity_type: "Person".into(),
    }];
    let item = MemoryItem::from_input(input, 1);
    backend.create(&ns, &item, None).await?;

    let indexer = Indexer::new(backend.clone())
        .with_vector(backend.clone())
        .with_graph(backend.clone());
    let report = indexer.run_pass().await?;

    let hits = recall::vector_search(
        backend.as_ref(),
        backend.as_ref(),
        &ns,
        &[1.0, 0.0],
        5,
        None,
    )
    .await?;
    println!(
        "level 4 indexed: {}, failed: {}, search hits: {}",
        report.indexed,
        report.failed,
        hits.len()
    );
    Ok(())
}
