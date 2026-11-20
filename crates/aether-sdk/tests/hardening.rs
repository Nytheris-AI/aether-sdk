use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use muninn_aether_adapters::{FaultPoint, InMemoryBackend};
use muninn_aether_core::{
    AetherError, ConsistencyMode, DistanceMetric, Entity, EntityId, MemoryInput, MemoryScope,
    MemoryUpdate, ProjectId, RecallMode, RecallQuery, ReceiptState, RecordStore, Relation,
    SessionId, UserId, VectorStore, WriteDurability,
};
use muninn_aether_sdk::MemoryManager;

fn test_ns() -> ProjectId {
    ProjectId::new(format!("t-hard-{}", uuid::Uuid::new_v4())).unwrap()
}

fn no_retry_manager(backend: Arc<InMemoryBackend>) -> MemoryManager {
    MemoryManager::builder(backend.clone())
        .vector(backend.clone())
        .graph(backend)
        .with_retry(None)
        .build()
}

#[tokio::test]
async fn failed_flush_loses_nothing() {
    let backend = Arc::new(InMemoryBackend::new());
    let ns = test_ns();
    backend
        .initialize(&ns, 2, DistanceMetric::Cosine)
        .await
        .unwrap();
    let mgr = no_retry_manager(backend.clone());

    let mut in1 = MemoryInput::new(ns.clone(), "memory one");
    in1.embedding = Some(vec![1.0, 0.0]);
    let mut in2 = MemoryInput::new(ns.clone(), "memory two");
    in2.embedding = Some(vec![0.0, 1.0]);

    let r1 = mgr.remember(in1).await.unwrap();
    let r2 = mgr.remember(in2).await.unwrap();
    assert_eq!(r1.state, ReceiptState::Buffered);
    assert_eq!(r2.state, ReceiptState::Buffered);

    backend.fail_next(FaultPoint::VectorUpsert, 1);
    assert!(mgr.flush().await.is_err());
    assert!(!backend.has_vector(&ns, r1.memory_id));
    assert!(backend.has_vector(&ns, r2.memory_id));

    mgr.flush().await.unwrap();
    assert!(backend.has_vector(&ns, r1.memory_id));
}

#[tokio::test]
async fn buffered_writes_survive_a_rebuilt_manager() {
    let backend = Arc::new(InMemoryBackend::new());
    let ns = test_ns();
    backend
        .initialize(&ns, 2, DistanceMetric::Cosine)
        .await
        .unwrap();

    let mut input = MemoryInput::new(ns.clone(), "durable buffered");
    input.embedding = Some(vec![1.0, 0.0]);
    let receipt = {
        let mgr1 = no_retry_manager(backend.clone());
        mgr1.remember(input).await.unwrap()
    };
    assert!(!backend.has_vector(&ns, receipt.memory_id));

    let mgr2 = no_retry_manager(backend.clone());
    mgr2.flush().await.unwrap();
    assert!(backend.has_vector(&ns, receipt.memory_id));
}

#[tokio::test]
async fn buffered_writes_are_not_indexed_without_opt_in_and_are_with_opt_in() {
    let backend = Arc::new(InMemoryBackend::new());
    let ns = test_ns();
    backend
        .initialize(&ns, 2, DistanceMetric::Cosine)
        .await
        .unwrap();

    let mgr_manual = no_retry_manager(backend.clone());
    let mut in1 = MemoryInput::new(ns.clone(), "manual");
    in1.embedding = Some(vec![1.0, 0.0]);
    let r1 = mgr_manual.remember(in1).await.unwrap();
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(!backend.has_vector(&ns, r1.memory_id));

    let mgr_bg = MemoryManager::builder(backend.clone())
        .vector(backend.clone())
        .background_indexing(Some(Duration::from_millis(10)))
        .build();
    let mut in2 = MemoryInput::new(ns.clone(), "auto indexed");
    in2.embedding = Some(vec![0.0, 1.0]);
    let r2 = mgr_bg.remember(in2).await.unwrap();

    for _ in 0..50 {
        if backend.has_vector(&ns, r2.memory_id) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(backend.has_vector(&ns, r2.memory_id));
    mgr_bg.shutdown().await.unwrap();
}

#[tokio::test]
async fn forgetting_an_unflushed_memory_does_not_resurrect_it() {
    let backend = Arc::new(InMemoryBackend::new());
    let ns = test_ns();
    backend
        .initialize(&ns, 2, DistanceMetric::Cosine)
        .await
        .unwrap();
    let mgr = no_retry_manager(backend.clone());

    let mut input = MemoryInput::new(ns.clone(), "transient buffered");
    input.embedding = Some(vec![1.0, 0.0]);
    let r = mgr.remember(input).await.unwrap();
    mgr.forget(&ns, r.memory_id).await.unwrap();
    mgr.flush().await.unwrap();

    assert!(!backend.has_vector(&ns, r.memory_id));
    assert!(backend.get(&ns, r.memory_id).await.unwrap().is_none());
}

#[tokio::test]
async fn failed_forget_hides_the_memory_and_is_retried() {
    let backend = Arc::new(InMemoryBackend::new());
    let ns = test_ns();
    backend
        .initialize(&ns, 2, DistanceMetric::Cosine)
        .await
        .unwrap();
    let mgr = no_retry_manager(backend.clone());

    let mut input = MemoryInput::new(ns.clone(), "forget me");
    input.embedding = Some(vec![1.0, 0.0]);
    input.durability = WriteDurability::WaitFlush;
    let r = mgr.remember(input).await.unwrap();

    backend.fail_next(FaultPoint::VectorDelete, 1);
    assert!(mgr.forget(&ns, r.memory_id).await.is_err());

    // Recall must hide tombstoned memory even before VectorDelete succeeds.
    let mut q = RecallQuery::new(ns.clone(), "forget");
    q.query_vector = Some(vec![1.0, 0.0]);
    q.mode = RecallMode::Vector;
    assert!(mgr.recall(q).await.unwrap().hits.is_empty());

    mgr.flush().await.unwrap();
    assert!(!backend.has_vector(&ns, r.memory_id));
}

#[tokio::test]
async fn unflushed_and_flushed_copies_score_the_same() {
    let backend = Arc::new(InMemoryBackend::new());
    let ns = test_ns();
    backend
        .initialize(&ns, 2, DistanceMetric::Euclidean)
        .await
        .unwrap();
    let mgr = no_retry_manager(backend.clone());

    let mut input = MemoryInput::new(ns.clone(), "same score");
    input.embedding = Some(vec![3.0, 4.0]);
    mgr.remember(input).await.unwrap();

    let mut q = RecallQuery::new(ns.clone(), "q");
    q.query_vector = Some(vec![0.0, 0.0]);
    q.mode = RecallMode::Vector;

    let before_flush = mgr.recall(q.clone()).await.unwrap().hits[0].score;
    mgr.flush().await.unwrap();
    let after_flush = mgr.recall(q).await.unwrap().hits[0].score;
    assert!((before_flush - after_flush).abs() < 1e-5);
}

#[tokio::test]
async fn an_unrelated_unflushed_write_does_not_outrank_a_real_match() {
    let backend = Arc::new(InMemoryBackend::new());
    let ns = test_ns();
    backend
        .initialize(&ns, 2, DistanceMetric::Cosine)
        .await
        .unwrap();
    let mgr = no_retry_manager(backend.clone());

    let mut good = MemoryInput::new(ns.clone(), "exact match");
    good.embedding = Some(vec![1.0, 0.0]);
    good.durability = WriteDurability::WaitFlush;
    let r_good = mgr.remember(good).await.unwrap();

    let mut unrelated = MemoryInput::new(ns.clone(), "orthogonal buffered");
    unrelated.embedding = Some(vec![0.0, 1.0]);
    mgr.remember(unrelated).await.unwrap();

    let mut q = RecallQuery::new(ns, "q");
    q.query_vector = Some(vec![1.0, 0.0]);
    q.mode = RecallMode::Vector;
    let hits = mgr.recall(q).await.unwrap().hits;
    assert_eq!(hits[0].memory_id, r_good.memory_id);
}

#[tokio::test]
async fn eventual_consistency_skips_unflushed_writes() {
    let backend = Arc::new(InMemoryBackend::new());
    let ns = test_ns();
    backend
        .initialize(&ns, 2, DistanceMetric::Cosine)
        .await
        .unwrap();
    let mgr = no_retry_manager(backend.clone());

    let mut input = MemoryInput::new(ns.clone(), "unflushed");
    input.embedding = Some(vec![1.0, 0.0]);
    mgr.remember(input).await.unwrap();

    let mut q = RecallQuery::new(ns, "q");
    q.query_vector = Some(vec![1.0, 0.0]);
    q.mode = RecallMode::Vector;
    q.consistency = ConsistencyMode::Eventual;
    assert!(mgr.recall(q).await.unwrap().hits.is_empty());
}

#[tokio::test]
async fn top_k_zero_returns_empty() {
    let backend = Arc::new(InMemoryBackend::new());
    let ns = test_ns();
    backend
        .initialize(&ns, 2, DistanceMetric::Cosine)
        .await
        .unwrap();
    let mgr = no_retry_manager(backend.clone());

    let mut input = MemoryInput::new(ns.clone(), "item");
    input.embedding = Some(vec![1.0, 0.0]);
    input.durability = WriteDurability::WaitFlush;
    mgr.remember(input).await.unwrap();

    let mut q = RecallQuery::new(ns, "q");
    q.query_vector = Some(vec![1.0, 0.0]);
    q.mode = RecallMode::Vector;
    q.top_k = 0;
    assert!(mgr.recall(q).await.unwrap().hits.is_empty());
}

#[tokio::test]
async fn hybrid_surfaces_a_graph_connected_memory_the_vector_search_missed() {
    let backend = Arc::new(InMemoryBackend::new());
    let ns = test_ns();
    backend
        .initialize(&ns, 2, DistanceMetric::Cosine)
        .await
        .unwrap();
    let mgr = no_retry_manager(backend.clone());

    let alice = Entity {
        id: EntityId::new("person:alice").unwrap(),
        name: "Alice".into(),
        entity_type: "Person".into(),
    };
    let bob = Entity {
        id: EntityId::new("person:bob").unwrap(),
        name: "Bob".into(),
        entity_type: "Person".into(),
    };

    let mut m1 = MemoryInput::new(ns.clone(), "Alice works with Bob");
    m1.embedding = Some(vec![1.0, 0.0]);
    m1.durability = WriteDurability::WaitFlush;
    m1.entities = vec![alice.clone(), bob.clone()];
    m1.relations = vec![Relation {
        source: alice.id.clone(),
        target: bob.id.clone(),
        relation_type: "WORKS_WITH".into(),
        valid_at: Utc::now(),
        invalid_at: None,
    }];
    mgr.remember(m1).await.unwrap();

    // Graph-only memory (no embedding, so vector search cannot return it).
    let mut m2 = MemoryInput::new(ns.clone(), "Bob leads Infra");
    m2.durability = WriteDurability::WaitFlush;
    m2.entities = vec![bob.clone()];
    m2.relations = vec![Relation {
        source: bob.id.clone(),
        target: EntityId::new("team:infra").unwrap(),
        relation_type: "LEADS".into(),
        valid_at: Utc::now(),
        invalid_at: None,
    }];
    let r2 = mgr.remember(m2).await.unwrap();

    let mut q = RecallQuery::new(ns, "Alice");
    q.query_vector = Some(vec![1.0, 0.0]);
    q.mode = RecallMode::Hybrid;
    q.top_k = 5;
    let res = mgr.recall(q).await.unwrap();
    assert!(res.hits.iter().any(|h| h.memory_id == r2.memory_id));
}

#[tokio::test]
async fn one_user_cannot_recall_another_users_memory() {
    let backend = Arc::new(InMemoryBackend::new());
    let ns = test_ns();
    backend
        .initialize(&ns, 2, DistanceMetric::Cosine)
        .await
        .unwrap();
    let mgr = no_retry_manager(backend.clone());

    let u1 = UserId::new("alice").unwrap();
    let u2 = UserId::new("bob").unwrap();

    let mut in_u1 = MemoryInput::new(ns.clone(), "alice private");
    in_u1.scope = MemoryScope::User(u1.clone());
    in_u1.embedding = Some(vec![1.0, 0.0]);
    in_u1.durability = WriteDurability::WaitFlush;
    let r1 = mgr.remember(in_u1).await.unwrap();

    let mut in_u2 = MemoryInput::new(ns.clone(), "bob private");
    in_u2.scope = MemoryScope::User(u2);
    in_u2.embedding = Some(vec![1.0, 0.0]);
    in_u2.durability = WriteDurability::WaitFlush;
    mgr.remember(in_u2).await.unwrap();

    let mut q = RecallQuery::new(ns, "private");
    q.query_vector = Some(vec![1.0, 0.0]);
    q.mode = RecallMode::Vector;
    q.scopes = vec![MemoryScope::User(u1)];
    let hits = mgr.recall(q).await.unwrap().hits;
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].memory_id, r1.memory_id);
}

#[tokio::test]
async fn session_memory_is_recalled_while_live_never_indexed_and_gone_after_ttl() {
    let backend = Arc::new(InMemoryBackend::new());
    let ns = test_ns();
    backend
        .initialize(&ns, 2, DistanceMetric::Cosine)
        .await
        .unwrap();
    let mgr = MemoryManager::builder(backend.clone())
        .vector(backend.clone())
        .with_session_ttl(Some(Duration::from_millis(25)))
        .with_retry(None)
        .build();

    let sid = SessionId::new("sess-1").unwrap();
    let mut input = MemoryInput::new(ns.clone(), "ephemeral turn");
    input.scope = MemoryScope::Session(sid.clone());
    input.embedding = Some(vec![1.0, 0.0]);
    input.durability = WriteDurability::WaitFlush;
    let r = mgr.remember(input).await.unwrap();
    assert!(!backend.has_vector(&ns, r.memory_id));

    let mut q = RecallQuery::new(ns, "turn");
    q.query_vector = Some(vec![1.0, 0.0]);
    q.mode = RecallMode::Vector;
    q.scopes = vec![MemoryScope::Session(sid)];
    assert_eq!(mgr.recall(q.clone()).await.unwrap().hits.len(), 1);

    tokio::time::sleep(Duration::from_millis(40)).await;
    assert!(mgr.recall(q).await.unwrap().hits.is_empty());
}

#[tokio::test]
async fn wrong_dimension_and_uninitialized_project_are_rejected() {
    let backend = Arc::new(InMemoryBackend::new());
    let ns_init = test_ns();
    let ns_uninit = test_ns();
    backend
        .initialize(&ns_init, 3, DistanceMetric::Cosine)
        .await
        .unwrap();
    let mgr = no_retry_manager(backend);

    let mut bad_dim = MemoryInput::new(ns_init, "bad dim");
    bad_dim.embedding = Some(vec![1.0, 0.0]);
    assert!(matches!(
        mgr.remember(bad_dim).await,
        Err(AetherError::InvalidInput(_))
    ));

    let mut uninit = MemoryInput::new(ns_uninit, "uninit");
    uninit.embedding = Some(vec![1.0, 0.0]);
    assert!(matches!(
        mgr.remember(uninit).await,
        Err(AetherError::InvalidInput(_))
    ));
}

#[tokio::test]
async fn update_content_requires_new_embedding_and_replaces_facts() {
    let backend = Arc::new(InMemoryBackend::new());
    let ns = test_ns();
    backend
        .initialize(&ns, 2, DistanceMetric::Cosine)
        .await
        .unwrap();
    let mgr = no_retry_manager(backend.clone());

    let alice = Entity {
        id: EntityId::new("person:alice").unwrap(),
        name: "Alice".into(),
        entity_type: "Person".into(),
    };
    let mut input = MemoryInput::new(ns.clone(), "Alice likes Qdrant");
    input.embedding = Some(vec![1.0, 0.0]);
    input.durability = WriteDurability::WaitFlush;
    input.entities = vec![alice.clone()];
    input.relations = vec![Relation {
        source: alice.id.clone(),
        target: EntityId::new("db:qdrant").unwrap(),
        relation_type: "LIKES".into(),
        valid_at: Utc::now(),
        invalid_at: None,
    }];
    let r = mgr.remember(input).await.unwrap();

    let missing_emb = mgr
        .update(
            &ns,
            r.memory_id,
            MemoryUpdate {
                content: Some("Alice likes pgvector".into()),
                embedding: None,
                metadata: None,
                entities: None,
                relations: None,
                expected_version: Some(1),
            },
        )
        .await;
    assert!(matches!(missing_emb, Err(AetherError::InvalidInput(_))));

    let pg = EntityId::new("db:pgvector").unwrap();
    mgr.update(
        &ns,
        r.memory_id,
        MemoryUpdate {
            content: Some("Alice likes pgvector".into()),
            embedding: Some(vec![0.0, 1.0]),
            metadata: None,
            entities: Some(vec![alice.clone()]),
            relations: Some(vec![Relation {
                source: alice.id.clone(),
                target: pg.clone(),
                relation_type: "LIKES".into(),
                valid_at: Utc::now(),
                invalid_at: None,
            }]),
            expected_version: Some(1),
        },
    )
    .await
    .unwrap();

    let mut q = RecallQuery::new(ns, "Alice");
    q.query_vector = Some(vec![0.0, 1.0]);
    q.seed_entities = vec![alice.id];
    q.mode = RecallMode::Hybrid;
    let res = mgr.recall(q).await.unwrap();
    assert_eq!(res.subgraph.edges.len(), 1);
    assert_eq!(res.subgraph.edges[0].target, pg);
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_remembers_with_one_key_create_one_memory() {
    let backend = Arc::new(InMemoryBackend::new());
    let ns = test_ns();
    backend
        .initialize(&ns, 2, DistanceMetric::Cosine)
        .await
        .unwrap();
    let mgr = Arc::new(no_retry_manager(backend));

    let mut handles = Vec::new();
    for i in 0..16 {
        let mgr = Arc::clone(&mgr);
        let ns = ns.clone();
        handles.push(tokio::spawn(async move {
            let mut input = MemoryInput::new(ns, format!("dup {i}"));
            input.idempotency_key = Some("same-key".into());
            input.embedding = Some(vec![1.0, 0.0]);
            mgr.remember(input).await.unwrap().memory_id
        }));
    }

    let mut ids = Vec::new();
    for h in handles {
        ids.push(h.await.unwrap());
    }
    assert!(ids.iter().all(|id| *id == ids[0]));
}

#[tokio::test]
async fn record_only_manager_works_and_errors_on_vector_or_graph_recall() {
    let backend = Arc::new(InMemoryBackend::new());
    let ns = test_ns();
    let mgr = MemoryManager::builder(backend).build();

    let sid = SessionId::new("sess-rec-only").unwrap();
    let mut input = MemoryInput::new(ns.clone(), "session note");
    input.scope = MemoryScope::Session(sid.clone());
    input.embedding = Some(vec![1.0, 0.0]);
    let r = mgr.remember(input).await.unwrap();
    assert_eq!(r.state, ReceiptState::Buffered);

    let mut vec_q = RecallQuery::new(ns.clone(), "q");
    vec_q.query_vector = Some(vec![1.0, 0.0]);
    vec_q.mode = RecallMode::Vector;
    assert!(matches!(
        mgr.recall(vec_q).await,
        Err(AetherError::InvalidInput(_))
    ));

    let mut graph_q = RecallQuery::new(ns, "q");
    graph_q.seed_entities = vec![EntityId::new("person:a").unwrap()];
    graph_q.mode = RecallMode::Graph;
    assert!(matches!(
        mgr.recall(graph_q).await,
        Err(AetherError::InvalidInput(_))
    ));
}

#[tokio::test]
async fn default_manager_retries_transient_connector_failures() {
    let backend = Arc::new(InMemoryBackend::new());
    let ns = test_ns();
    backend
        .initialize(&ns, 2, DistanceMetric::Cosine)
        .await
        .unwrap();
    let mgr = MemoryManager::builder(backend.clone())
        .vector(backend.clone())
        .build();

    backend.fail_next(FaultPoint::VectorUpsert, 2);
    let mut input = MemoryInput::new(ns.clone(), "retried write");
    input.embedding = Some(vec![1.0, 0.0]);
    input.durability = WriteDurability::WaitFlush;
    let r = mgr.remember(input).await.unwrap();
    assert_eq!(r.state, ReceiptState::Persisted);
    assert!(backend.has_vector(&ns, r.memory_id));
}

#[tokio::test]
async fn hybrid_combines_explicit_seeds_with_vector_hit_entities_and_relations() {
    let backend = Arc::new(InMemoryBackend::new());
    let ns = test_ns();
    backend
        .initialize(&ns, 2, DistanceMetric::Cosine)
        .await
        .unwrap();
    let mgr = no_retry_manager(backend);

    let alice = Entity {
        id: EntityId::new("person:alice").unwrap(),
        name: "Alice".into(),
        entity_type: "Person".into(),
    };
    let bob = Entity {
        id: EntityId::new("person:bob").unwrap(),
        name: "Bob".into(),
        entity_type: "Person".into(),
    };
    let carol = Entity {
        id: EntityId::new("person:carol").unwrap(),
        name: "Carol".into(),
        entity_type: "Person".into(),
    };

    let mut m_vec = MemoryInput::new(ns.clone(), "Alice partnered with Bob");
    m_vec.embedding = Some(vec![1.0, 0.0]);
    m_vec.durability = WriteDurability::WaitFlush;
    m_vec.entities = vec![alice.clone(), bob.clone()];
    mgr.remember(m_vec).await.unwrap();

    let mut m_bob_neighbor = MemoryInput::new(ns.clone(), "Bob leads core");
    m_bob_neighbor.durability = WriteDurability::WaitFlush;
    m_bob_neighbor.relations = vec![Relation {
        source: bob.id.clone(),
        target: EntityId::new("team:core").unwrap(),
        relation_type: "LEADS".into(),
        valid_at: Utc::now(),
        invalid_at: None,
    }];
    let r_bob = mgr.remember(m_bob_neighbor).await.unwrap();

    let mut m_carol_neighbor = MemoryInput::new(ns.clone(), "Carol leads auth");
    m_carol_neighbor.durability = WriteDurability::WaitFlush;
    m_carol_neighbor.entities = vec![carol.clone()];
    m_carol_neighbor.relations = vec![Relation {
        source: carol.id.clone(),
        target: EntityId::new("team:auth").unwrap(),
        relation_type: "LEADS".into(),
        valid_at: Utc::now(),
        invalid_at: None,
    }];
    let r_carol = mgr.remember(m_carol_neighbor).await.unwrap();

    // Even when explicit seed_entities only contains carol, vector-hit entity `bob` is also expanded.
    let mut q = RecallQuery::new(ns, "partner");
    q.query_vector = Some(vec![1.0, 0.0]);
    q.seed_entities = vec![carol.id];
    q.mode = RecallMode::Hybrid;
    q.top_k = 10;
    let res = mgr.recall(q).await.unwrap();
    assert!(res.hits.iter().any(|h| h.memory_id == r_bob.memory_id));
    assert!(res.hits.iter().any(|h| h.memory_id == r_carol.memory_id));
}

#[tokio::test]
async fn session_recall_with_default_scopes_does_not_fail_when_vector_index_uninitialized() {
    let backend = Arc::new(InMemoryBackend::new());
    let ns = test_ns();
    // Note: backend.initialize is NOT called.
    let mgr = MemoryManager::builder(backend.clone())
        .vector(backend)
        .with_retry(None)
        .build();

    let sid = SessionId::new("sess-uninit").unwrap();
    let mut input = MemoryInput::new(ns.clone(), "session turn");
    input.scope = MemoryScope::Session(sid.clone());
    input.embedding = Some(vec![1.0, 0.0]);
    let r = mgr.remember(input).await.unwrap();

    let mut q = RecallQuery::new(ns, "turn");
    q.query_vector = Some(vec![1.0, 0.0]);
    q.mode = RecallMode::Vector;
    q.scopes = vec![MemoryScope::Project, MemoryScope::Session(sid)];
    let res = mgr.recall(q).await.unwrap();
    assert_eq!(res.hits.len(), 1);
    assert_eq!(res.hits[0].memory_id, r.memory_id);
}

#[tokio::test]
async fn remember_after_shutdown_returns_invalid_input() {
    let backend = Arc::new(InMemoryBackend::new());
    let ns = test_ns();
    backend
        .initialize(&ns, 2, DistanceMetric::Cosine)
        .await
        .unwrap();
    let mgr = no_retry_manager(backend);
    mgr.shutdown().await.unwrap();

    let mut input = MemoryInput::new(ns, "after shutdown");
    input.embedding = Some(vec![1.0, 0.0]);
    let err = mgr.remember(input).await.unwrap_err();
    assert!(matches!(err, AetherError::InvalidInput(_)));
}
