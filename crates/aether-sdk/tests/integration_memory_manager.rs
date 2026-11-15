use std::sync::Arc;

use aether_adapters::InMemoryBackend;
use aether_core::{
    AetherError, DistanceMetric, Entity, EntityId, MemoryInbound, MemoryInput, MemoryUpdate,
    ProjectId, ReceiptState, RecallMode, RecallQuery, Relation, TemporalFilter, VectorStore,
    GraphStore, WriteDurability,
};
use aether_sdk::MemoryManager;
use chrono::Utc;

#[tokio::test]
async fn remember_recall_update_cas_and_forget_lifecycle() {
    let backend = Arc::new(InMemoryBackend::new());
    let pid = ProjectId::new("proj-sdk").unwrap();
    backend.initialize(&pid, 3, DistanceMetric::Cosine).await.unwrap();

    let manager: Arc<dyn MemoryInbound> = Arc::new(MemoryManager::new(
        backend.clone(),
        backend.clone(),
        backend.clone(),
    ));

    let mut input = MemoryInput::new(pid.clone(), "Alice uses Turbopuffer for vector recall.");
    input.embedding = Some(vec![1.0, 0.0, 0.0]);
    input.durability = WriteDurability::WaitFlush;
    input.entities = vec![Entity {
        id: EntityId::new("person:alice").unwrap(),
        name: "Alice".into(),
        entity_type: "Person".into(),
    }];
    input.relations = vec![Relation {
        source: EntityId::new("person:alice").unwrap(),
        target: EntityId::new("db:turbopuffer").unwrap(),
        relation_type: "USES".into(),
        valid_at: Utc::now(),
        invalid_at: None,
    }];

    let receipt = manager.remember(input).await.unwrap();
    assert_eq!(receipt.state, ReceiptState::Persisted);
    assert_eq!(receipt.version, 1);

    // Hybrid recall retrieves both vector hit and graph neighborhood.
    let mut query = RecallQuery::new(pid.clone(), "Alice vector database");
    query.query_vector = Some(vec![1.0, 0.0, 0.0]);
    query.seed_entities = vec![EntityId::new("person:alice").unwrap()];
    query.mode = RecallMode::Hybrid;

    let recalled = manager.recall(query).await.unwrap();
    assert_eq!(recalled.hits.len(), 1);
    assert_eq!(recalled.hits[0].memory_id, receipt.memory_id);
    assert_eq!(recalled.subgraph.edges.len(), 1);

    // CAS update succeeds with expected_version = 1, fails with stale version = 1 afterward.
    let updated = manager
        .update(
            &pid,
            receipt.memory_id,
            MemoryUpdate {
                content: Some("Alice uses Turbopuffer and Neo4j.".into()),
                embedding: None,
                metadata: None,
                expected_version: Some(1),
            },
        )
        .await
        .unwrap();
    assert_eq!(updated.version, 2);

    let conflict = manager
        .update(
            &pid,
            receipt.memory_id,
            MemoryUpdate {
                content: Some("stale write".into()),
                embedding: None,
                metadata: None,
                expected_version: Some(1),
            },
        )
        .await;
    assert!(matches!(conflict, Err(AetherError::Conflict { .. })));

    // forget() removes vector record AND retracts graph provenance edges.
    manager.forget(&pid, receipt.memory_id).await.unwrap();

    let vec_after = backend.search(&pid, &[1.0, 0.0, 0.0], 5, None).await.unwrap();
    assert!(vec_after.is_empty());

    let graph_after = backend
        .neighborhood(
            &pid,
            &[EntityId::new("person:alice").unwrap()],
            1,
            10,
            &TemporalFilter::default(),
        )
        .await
        .unwrap();
    assert!(graph_after.edges.is_empty());

    manager.shutdown().await.unwrap();
}
