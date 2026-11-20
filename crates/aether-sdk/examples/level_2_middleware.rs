//! Level 2: Stack resilience middleware (`Resilient` -> `Timeout` -> `Batcher`) around a connector.

use std::sync::Arc;
use std::time::Duration;

use muninn_aether_adapters::{
    Batcher, FaultPoint, InMemoryBackend, Resilient, RetryPolicy, Timeout,
};
use muninn_aether_core::{DistanceMetric, MemoryId, ProjectId, VectorRecord, VectorStore};
use serde_json::json;

#[tokio::main]
async fn main() -> muninn_aether_core::Result<()> {
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

    // Simulate a transient outage that recovers automatically on retry.
    raw.fail_next(FaultPoint::VectorUpsert, 1);

    let id = MemoryId::new();
    store
        .upsert_batch(
            &ns,
            &[VectorRecord {
                id,
                vector: vec![1.0, 0.0],
                metadata: json!({"retry": true}),
            }],
        )
        .await?;

    let hits = store.search(&ns, &[1.0, 0.0], 5, None).await?;
    println!("level 2 hits after retry: {}", hits.len());
    Ok(())
}
