use std::sync::Arc;

use aether_adapters::InMemoryBackend;
use aether_core::{
    DistanceMetric, Entity, EntityId, GraphEdge, MemoryId, MemoryInput, MemoryItem, ProjectId,
    ReceiptState, RecallMode, RecallQuery, Relation, Subgraph, VectorStore, WriteDurability,
};
use aether_cortex::{
    HotContextCache, IngestionBuffer, PersonalizedPageRank, PolicyDecision, PolicyRouter,
    QueryPlanner,
};
use chrono::Utc;

#[test]
fn policy_router_validates_and_deduplicates_idempotency_keys() {
    let router = PolicyRouter::new();
    let pid = ProjectId::new("proj-1").unwrap();

    let mut input1 = MemoryInput::new(pid.clone(), "Alice uses Qdrant.");
    input1.idempotency_key = Some("evt-123".into());

    let first = router.evaluate(&input1).unwrap();
    let first_id = match first {
        PolicyDecision::Accept(item) => item.id,
        PolicyDecision::Duplicate(_) => panic!("expected Accept on first seen key"),
    };

    let second = router.evaluate(&input1).unwrap();
    match second {
        PolicyDecision::Duplicate(receipt) => {
            assert_eq!(receipt.memory_id, first_id);
        }
        PolicyDecision::Accept(_) => panic!("expected Duplicate on repeated idempotency_key"),
    }

    // Same key in a different project is NOT a duplicate.
    let pid2 = ProjectId::new("proj-2").unwrap();
    let mut input_other_tenant = MemoryInput::new(pid2, "Alice uses Qdrant.");
    input_other_tenant.idempotency_key = Some("evt-123".into());
    assert!(matches!(
        router.evaluate(&input_other_tenant).unwrap(),
        PolicyDecision::Accept(_)
    ));
}

#[test]
fn hot_context_cache_enforces_capacity_and_project_isolation() {
    let cache = HotContextCache::new(2);
    let p1 = ProjectId::new("p1").unwrap();
    let p2 = ProjectId::new("p2").unwrap();

    let m1 = MemoryItem::from_input(MemoryInput::new(p1.clone(), "first"), 1);
    let m2 = MemoryItem::from_input(MemoryInput::new(p1.clone(), "second"), 1);
    let m3 = MemoryItem::from_input(MemoryInput::new(p1.clone(), "third"), 1);
    let other = MemoryItem::from_input(MemoryInput::new(p2.clone(), "other"), 1);

    cache.insert(m1.clone());
    cache.insert(m2.clone());
    cache.insert(m3.clone());
    cache.insert(other.clone());

    let p1_recent = cache.recent(&p1, 10);
    assert_eq!(p1_recent.len(), 2);
    assert_eq!(p1_recent[0].id, m3.id);
    assert_eq!(p1_recent[1].id, m2.id);

    let p2_recent = cache.recent(&p2, 10);
    assert_eq!(p2_recent.len(), 1);
    assert_eq!(p2_recent[0].id, other.id);
}

#[test]
fn personalized_page_rank_ranks_seed_neighborhood_higher() {
    let alice = EntityId::new("person:alice").unwrap();
    let turbopuffer = EntityId::new("db:turbopuffer").unwrap();
    let rust = EntityId::new("lang:rust").unwrap();
    let isolated = EntityId::new("other:node").unwrap();

    let now = Utc::now();
    let mid = MemoryId::new();
    let subgraph = Subgraph {
        entities: vec![
            Entity {
                id: alice.clone(),
                name: "Alice".into(),
                entity_type: "Person".into(),
            },
            Entity {
                id: turbopuffer.clone(),
                name: "Turbopuffer".into(),
                entity_type: "Database".into(),
            },
            Entity {
                id: rust.clone(),
                name: "Rust".into(),
                entity_type: "Language".into(),
            },
            Entity {
                id: isolated.clone(),
                name: "Isolated".into(),
                entity_type: "Unknown".into(),
            },
        ],
        edges: vec![
            GraphEdge {
                source: alice.clone(),
                target: turbopuffer.clone(),
                relation_type: "USES".into(),
                source_memory_id: mid,
                valid_at: now,
                invalid_at: None,
            },
            GraphEdge {
                source: turbopuffer.clone(),
                target: rust.clone(),
                relation_type: "WRITTEN_FOR".into(),
                source_memory_id: mid,
                valid_at: now,
                invalid_at: None,
            },
        ],
    };

    let ranks = PersonalizedPageRank::default().rank(&subgraph, &[alice.clone()]);
    let score_of = |id: &EntityId| {
        ranks
            .iter()
            .find(|(e, _)| e == id)
            .map(|(_, s)| *s)
            .unwrap_or(0.0)
    };

    assert!(score_of(&alice) > score_of(&turbopuffer));
    assert!(score_of(&turbopuffer) > score_of(&isolated));
}

#[tokio::test]
async fn ingestion_buffer_and_query_planner_support_read_your_writes_and_flush() {
    let backend = Arc::new(InMemoryBackend::new());
    let pid = ProjectId::new("proj-ryw").unwrap();
    backend.initialize(&pid, 3, DistanceMetric::Cosine).await.unwrap();

    let cache = Arc::new(HotContextCache::new(32));
    let buffer = Arc::new(IngestionBuffer::new(backend.clone(), backend.clone()));

    let mut input = MemoryInput::new(pid.clone(), "Alice uses pgvector for hybrid search.");
    input.embedding = Some(vec![1.0, 0.0, 0.0]);
    input.durability = WriteDurability::Buffered;
    input.entities = vec![Entity {
        id: EntityId::new("person:alice").unwrap(),
        name: "Alice".into(),
        entity_type: "Person".into(),
    }];
    input.relations = vec![Relation {
        source: EntityId::new("person:alice").unwrap(),
        target: EntityId::new("db:pgvector").unwrap(),
        relation_type: "USES".into(),
        valid_at: Utc::now(),
        invalid_at: None,
    }];

    let item = MemoryItem::from_input(input, 1);
    cache.insert(item.clone());
    let receipt = buffer.enqueue(item.clone(), WriteDurability::Buffered).await.unwrap();
    assert_eq!(receipt.state, ReceiptState::Buffered);

    // Before flush(), ReadYourWrites recall still finds the buffered memory!
    let planner = QueryPlanner::new(backend.clone(), backend.clone(), cache.clone(), buffer.clone());
    let mut query = RecallQuery::new(pid.clone(), "What does Alice use?");
    query.query_vector = Some(vec![1.0, 0.0, 0.0]);
    query.mode = RecallMode::Hybrid;

    let pre_flush = planner.plan_and_execute(&query).await.unwrap();
    assert!(pre_flush.hits.iter().any(|h| h.memory_id == item.id));

    // After flush(), outbound VectorStore + GraphStore both hold the persisted data.
    buffer.flush().await.unwrap();
    let vec_hits = backend.search(&pid, &[1.0, 0.0, 0.0], 5, None).await.unwrap();
    assert_eq!(vec_hits.len(), 1);
    assert_eq!(vec_hits[0].id, item.id);
}
