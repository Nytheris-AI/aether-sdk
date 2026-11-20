use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use muninn_aether_adapters::{FaultPoint, InMemoryBackend};
use muninn_aether_core::{
    DistanceMetric, Entity, EntityId, GraphStore, MemoryInput, MemoryItem, MemoryScope, MemoryTier,
    PendingRecord, ProjectId, RecordStore, Relation, SessionId, TemporalFilter, VectorStore,
};
use muninn_aether_cortex::indexer::{index_metadata, Indexer};
use serde_json::json;

fn test_ns() -> ProjectId {
    ProjectId::new(format!("t-idx-{}", uuid::Uuid::new_v4())).unwrap()
}

fn make_item(ns: &ProjectId, content: &str, embedding: Option<Vec<f32>>) -> MemoryItem {
    let mut input = MemoryInput::new(ns.clone(), content);
    input.embedding = embedding;
    MemoryItem::from_input(input, 1)
}

#[tokio::test]
async fn failed_index_write_keeps_the_record_pending_and_indexes_the_rest() {
    let backend = Arc::new(InMemoryBackend::new());
    let ns = test_ns();
    backend
        .initialize(&ns, 2, DistanceMetric::Cosine)
        .await
        .unwrap();

    let item1 = make_item(&ns, "first memory", Some(vec![1.0, 0.0]));
    let item2 = make_item(&ns, "second memory", Some(vec![0.0, 1.0]));
    backend.create(&ns, &item1, None).await.unwrap();
    backend.create(&ns, &item2, None).await.unwrap();

    let indexer = Indexer::new(backend.clone()).with_vector(backend.clone());
    backend.fail_next(FaultPoint::VectorUpsert, 1);

    let err = indexer.run_pass().await.unwrap_err();
    assert!(err.is_retryable());
    assert!(!backend.has_vector(&ns, item1.id));
    assert!(backend.has_vector(&ns, item2.id));

    let still_pending = backend.pending_for(&ns, 10).await.unwrap();
    assert_eq!(still_pending.len(), 1);
    assert_eq!(still_pending[0].id, item1.id);

    let report = indexer.run_pass().await.unwrap();
    assert_eq!(report.indexed, 1);
    assert_eq!(report.failed, 0);
    assert!(backend.has_vector(&ns, item1.id));
    assert!(backend.pending_for(&ns, 10).await.unwrap().is_empty());
}

#[tokio::test]
async fn tombstone_removes_vector_and_facts_then_the_record() {
    let backend = Arc::new(InMemoryBackend::new());
    let ns = test_ns();
    backend
        .initialize(&ns, 2, DistanceMetric::Cosine)
        .await
        .unwrap();

    let alice = Entity {
        id: EntityId::new("person:alice").unwrap(),
        name: "Alice".into(),
        entity_type: "Person".into(),
    };
    let db = Entity {
        id: EntityId::new("db:sqlite").unwrap(),
        name: "SQLite".into(),
        entity_type: "Database".into(),
    };
    let mut input = MemoryInput::new(ns.clone(), "Alice uses SQLite");
    input.embedding = Some(vec![1.0, 0.0]);
    input.entities = vec![alice.clone(), db.clone()];
    input.relations = vec![Relation {
        source: alice.id.clone(),
        target: db.id.clone(),
        relation_type: "USES".into(),
        valid_at: Utc::now(),
        invalid_at: None,
    }];
    let item = MemoryItem::from_input(input, 1);
    backend.create(&ns, &item, None).await.unwrap();

    let indexer = Indexer::new(backend.clone())
        .with_vector(backend.clone())
        .with_graph(backend.clone());
    assert_eq!(indexer.run_pass().await.unwrap().indexed, 1);
    assert!(backend.has_vector(&ns, item.id));

    let tomb = backend.tombstone(&ns, item.id).await.unwrap().unwrap();
    assert_eq!(tomb.version, 2);

    backend.fail_next(FaultPoint::VectorDelete, 1);
    assert!(indexer.run_pass().await.is_err());
    assert!(backend.has_vector(&ns, item.id));
    assert_eq!(backend.pending(10).await.unwrap().len(), 1);

    let report = indexer.run_pass().await.unwrap();
    assert_eq!(report.indexed, 1);
    assert!(!backend.has_vector(&ns, item.id));
    assert!(backend.pending(10).await.unwrap().is_empty());

    let sub = backend
        .neighborhood(&ns, &[alice.id], 1, 10, &TemporalFilter::default())
        .await
        .unwrap();
    assert!(sub.edges.is_empty());
}

#[tokio::test]
async fn item_without_embedding_has_its_stale_vector_removed() {
    let backend = Arc::new(InMemoryBackend::new());
    let ns = test_ns();
    backend
        .initialize(&ns, 2, DistanceMetric::Cosine)
        .await
        .unwrap();

    let mut item = make_item(&ns, "with vector", Some(vec![1.0, 0.0]));
    backend.create(&ns, &item, None).await.unwrap();
    let indexer = Indexer::new(backend.clone()).with_vector(backend.clone());
    indexer.run_pass().await.unwrap();
    assert!(backend.has_vector(&ns, item.id));

    item.embedding = None;
    item.version = 2;
    backend
        .cas_update(&ns, item.id, Some(1), &item)
        .await
        .unwrap();
    indexer.run_pass().await.unwrap();
    assert!(!backend.has_vector(&ns, item.id));
}

#[tokio::test]
async fn indexer_with_no_vector_or_graph_store_marks_records_indexed() {
    let backend = Arc::new(InMemoryBackend::new());
    let ns = test_ns();
    let item = make_item(&ns, "record only", Some(vec![1.0, 0.0]));
    backend.create(&ns, &item, None).await.unwrap();

    let indexer = Indexer::new(backend.clone());
    let report = indexer.run_pass().await.unwrap();
    assert_eq!(report.indexed, 1);
    assert!(backend.pending_for(&ns, 10).await.unwrap().is_empty());
}

#[test]
fn index_metadata_adds_tier_and_omits_content() {
    let ns = test_ns();
    let mut input = MemoryInput::new(ns.clone(), "secret body");
    input.tier = MemoryTier::Procedural;
    input.metadata = json!({"tag": "ops"});
    let item = MemoryItem::from_input(input, 1);

    let meta = index_metadata(&item);
    assert_eq!(meta.get("tag"), Some(&json!("ops")));
    assert_eq!(meta.get("_aether_tier"), Some(&json!("procedural")));
    assert_eq!(meta.get("_aether_scope"), Some(&json!("project")));
    assert!(meta.get("content").is_none());
}

#[tokio::test]
async fn session_items_are_marked_indexed_without_writing_to_vector_or_graph() {
    let backend = Arc::new(InMemoryBackend::new());
    let ns = test_ns();
    backend
        .initialize(&ns, 2, DistanceMetric::Cosine)
        .await
        .unwrap();

    let alice = Entity {
        id: EntityId::new("person:alice").unwrap(),
        name: "Alice".into(),
        entity_type: "Person".into(),
    };
    let db = Entity {
        id: EntityId::new("db:sqlite").unwrap(),
        name: "SQLite".into(),
        entity_type: "Database".into(),
    };
    let mut input = MemoryInput::new(ns.clone(), "session turn");
    input.scope = MemoryScope::Session(SessionId::new("sess-1").unwrap());
    input.embedding = Some(vec![1.0, 0.0]);
    input.entities = vec![alice.clone(), db.clone()];
    input.relations = vec![Relation {
        source: alice.id.clone(),
        target: db.id.clone(),
        relation_type: "USES".into(),
        valid_at: Utc::now(),
        invalid_at: None,
    }];
    let item = MemoryItem::from_input(input, 1);
    backend
        .create(&ns, &item, Some(Duration::from_secs(60)))
        .await
        .unwrap();

    let indexer = Indexer::new(backend.clone())
        .with_vector(backend.clone())
        .with_graph(backend.clone());
    let report = indexer.run_pass().await.unwrap();
    assert_eq!(report.indexed, 1);
    assert!(!backend.has_vector(&ns, item.id));
    assert!(backend.pending(10).await.unwrap().is_empty());

    let sub = backend
        .neighborhood(&ns, &[alice.id], 1, 10, &TemporalFilter::default())
        .await
        .unwrap();
    assert!(sub.edges.is_empty());
}

#[tokio::test]
async fn spawn_indexes_when_notified() {
    let backend = Arc::new(InMemoryBackend::new());
    let ns = test_ns();
    backend
        .initialize(&ns, 2, DistanceMetric::Cosine)
        .await
        .unwrap();
    let item = make_item(&ns, "spawned", Some(vec![1.0, 0.0]));
    backend.create(&ns, &item, None).await.unwrap();

    let indexer = Arc::new(Indexer::new(backend.clone()).with_vector(backend.clone()));
    let handle = indexer.spawn(Duration::from_secs(60));
    indexer.notify();

    for _ in 0..50 {
        if backend.has_vector(&ns, item.id) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(backend.has_vector(&ns, item.id));

    handle.abort();
    let _ = PendingRecord {
        item,
        tombstone: false,
    };
}
