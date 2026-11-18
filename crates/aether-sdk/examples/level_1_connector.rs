//! Level 1: Call a single storage connector directly without middleware, indexers, or managers.

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
