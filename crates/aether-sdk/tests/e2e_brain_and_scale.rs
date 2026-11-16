use std::sync::Arc;

use aether_adapters::{InMemoryBackend, Resilient, RetryPolicy, Timeout, Traced};
use aether_core::{
    DistanceMetric, MemoryInbound, MemoryInput, ProjectId, RecallMode, RecallQuery, VectorStore,
    WriteDurability,
};
use aether_sdk::MemoryManager;
use std::time::Duration;

#[tokio::test]
async fn e2e_concurrent_multi_tenant_ingestion_with_resilient_middleware() {
    let raw = Arc::new(InMemoryBackend::new());
    let record_store = Arc::new(Traced::new(Resilient::new(
        Timeout::new(raw.clone(), Duration::from_secs(2)),
        RetryPolicy::default(),
    )));
    let vec_store = Arc::new(Traced::new(Resilient::new(
        Timeout::new(raw.clone(), Duration::from_secs(2)),
        RetryPolicy::default(),
    )));
    let graph_store = Arc::new(Traced::new(Resilient::new(
        Timeout::new(raw.clone(), Duration::from_secs(2)),
        RetryPolicy::default(),
    )));

    let p1 = ProjectId::new("tenant-1").unwrap();
    let p2 = ProjectId::new("tenant-2").unwrap();
    vec_store
        .initialize(&p1, 2, DistanceMetric::Cosine)
        .await
        .unwrap();
    vec_store
        .initialize(&p2, 2, DistanceMetric::Cosine)
        .await
        .unwrap();

    let manager = Arc::new(MemoryManager::new(record_store, vec_store, graph_store));

    let mut handles = Vec::new();
    for i in 0..20 {
        let mgr = manager.clone();
        let pid = if i % 2 == 0 { p1.clone() } else { p2.clone() };
        handles.push(tokio::spawn(async move {
            let mut input = MemoryInput::new(pid, format!("Memory item {i}"));
            input.embedding = Some(vec![1.0, i as f32 * 0.01]);
            input.durability = WriteDurability::Buffered;
            mgr.remember(input).await.unwrap();
        }));
    }

    for h in handles {
        h.await.unwrap();
    }

    manager.flush().await.unwrap();

    let mut q1 = RecallQuery::new(p1, "Memory item");
    q1.query_vector = Some(vec![1.0, 0.0]);
    q1.mode = RecallMode::Vector;
    q1.top_k = 20;

    let res1 = manager.recall(q1).await.unwrap();
    assert_eq!(res1.hits.len(), 10);

    manager.shutdown().await.unwrap();
}
