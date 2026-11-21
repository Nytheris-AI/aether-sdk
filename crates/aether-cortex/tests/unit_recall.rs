use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use muninn_aether_adapters::InMemoryBackend;
use muninn_aether_core::{
    DistanceMetric, Entity, EntityId, MemoryInput, MemoryItem, MemoryScope, ProjectId, RecordStore,
    Relation, SessionId, TemporalFilter, UserId, VectorStore,
};
use muninn_aether_cortex::indexer::Indexer;
use muninn_aether_cortex::ppr::{graph_scores, PersonalizedPageRank};
use muninn_aether_cortex::recall::{
    graph_search, in_scopes, merge, rrf, scope_filter, session, unindexed, vector_search,
    Candidate, RRF_K,
};

fn test_ns() -> ProjectId {
    ProjectId::new(format!("t-rec-{}", uuid::Uuid::new_v4())).unwrap()
}

fn make_item(ns: &ProjectId, content: &str, embedding: Vec<f32>) -> MemoryItem {
    let mut input = MemoryInput::new(ns.clone(), content);
    input.embedding = Some(embedding);
    MemoryItem::from_input(input, 1)
}

#[tokio::test]
async fn vector_search_resolves_through_records_and_drops_tombstoned_hits() {
    let backend = Arc::new(InMemoryBackend::new());
    let ns = test_ns();
    backend
        .initialize(&ns, 2, DistanceMetric::Cosine)
        .await
        .unwrap();

    let live = make_item(&ns, "live memory", vec![1.0, 0.0]);
    let doomed = make_item(&ns, "doomed memory", vec![0.99, 0.05]);
    backend.create(&ns, &live, None).await.unwrap();
    backend.create(&ns, &doomed, None).await.unwrap();

    let indexer = Indexer::new(backend.clone()).with_vector(backend.clone());
    indexer.run_pass().await.unwrap();

    // Tombstone in record store only without running indexer yet; vector_search must still drop it.
    backend.tombstone(&ns, doomed.id).await.unwrap();

    let hits = vector_search(&*backend, &*backend, &ns, &[1.0, 0.0], 10, None)
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].item.id, live.id);
}

#[tokio::test]
async fn unindexed_scores_with_index_metric_and_merge_keeps_newest_version() {
    let backend = Arc::new(InMemoryBackend::new());
    let ns = test_ns();
    backend
        .initialize(&ns, 2, DistanceMetric::Euclidean)
        .await
        .unwrap();

    let mut v1 = make_item(&ns, "v1 text", vec![1.0, 0.0]);
    backend.create(&ns, &v1, None).await.unwrap();
    let indexer = Indexer::new(backend.clone()).with_vector(backend.clone());
    indexer.run_pass().await.unwrap();

    v1.content = "v2 text".into();
    v1.version = 2;
    backend.cas_update(&ns, v1.id, Some(1), &v1).await.unwrap();

    let unrelated = make_item(&ns, "far away", vec![10.0, 10.0]);
    backend.create(&ns, &unrelated, None).await.unwrap();

    let indexed = vector_search(&*backend, &*backend, &ns, &[1.0, 0.0], 5, None)
        .await
        .unwrap();
    let fresh = unindexed(&*backend, &ns, &[1.0, 0.0], DistanceMetric::Euclidean, None)
        .await
        .unwrap();

    let merged = merge(vec![indexed, fresh]);
    assert_eq!(merged.len(), 2);
    assert_eq!(merged[0].item.id, v1.id);
    assert_eq!(merged[0].item.version, 2);
    assert_eq!(merged[0].item.content, "v2 text");
    assert!(merged[0].score > merged[1].score);
}

#[tokio::test]
async fn session_step_returns_only_live_matching_session_items() {
    let backend = Arc::new(InMemoryBackend::new());
    let ns = test_ns();
    let s1 = SessionId::new("s-1").unwrap();
    let s2 = SessionId::new("s-2").unwrap();

    let mut m1_in = MemoryInput::new(ns.clone(), "s1 live");
    m1_in.scope = MemoryScope::Session(s1.clone());
    m1_in.embedding = Some(vec![1.0, 0.0]);
    let m1 = MemoryItem::from_input(m1_in, 1);

    let mut m1_exp_in = MemoryInput::new(ns.clone(), "s1 expired");
    m1_exp_in.scope = MemoryScope::Session(s1.clone());
    m1_exp_in.embedding = Some(vec![1.0, 0.0]);
    let m1_exp = MemoryItem::from_input(m1_exp_in, 1);

    let mut m2_in = MemoryInput::new(ns.clone(), "s2 live");
    m2_in.scope = MemoryScope::Session(s2);
    m2_in.embedding = Some(vec![1.0, 0.0]);
    let m2 = MemoryItem::from_input(m2_in, 1);

    backend.create(&ns, &m1, None).await.unwrap();
    backend
        .create(&ns, &m1_exp, Some(Duration::from_millis(20)))
        .await
        .unwrap();
    backend.create(&ns, &m2, None).await.unwrap();
    tokio::time::sleep(Duration::from_millis(35)).await;

    let res = session(
        &*backend,
        &ns,
        &s1,
        &[1.0, 0.0],
        DistanceMetric::Cosine,
        None,
    )
    .await
    .unwrap();
    assert_eq!(res.len(), 1);
    assert_eq!(res[0].item.id, m1.id);
}

#[tokio::test]
async fn graph_search_and_graph_scores_rank_memories_by_edge_ppr() {
    let backend = Arc::new(InMemoryBackend::new());
    let ns = test_ns();

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

    let mut m_near = MemoryInput::new(ns.clone(), "Alice knows Bob");
    m_near.entities = vec![alice.clone(), bob.clone()];
    m_near.relations = vec![Relation {
        source: alice.id.clone(),
        target: bob.id.clone(),
        relation_type: "KNOWS".into(),
        valid_at: Utc::now(),
        invalid_at: None,
    }];
    let m_near = MemoryItem::from_input(m_near, 1);

    let mut m_far = MemoryInput::new(ns.clone(), "Bob knows Carol");
    m_far.entities = vec![bob.clone(), carol.clone()];
    m_far.relations = vec![Relation {
        source: bob.id.clone(),
        target: carol.id.clone(),
        relation_type: "KNOWS".into(),
        valid_at: Utc::now(),
        invalid_at: None,
    }];
    let m_far = MemoryItem::from_input(m_far, 1);

    backend.create(&ns, &m_near, None).await.unwrap();
    backend.create(&ns, &m_far, None).await.unwrap();
    let indexer = Indexer::new(backend.clone()).with_graph(backend.clone());
    indexer.run_pass().await.unwrap();

    let ppr = PersonalizedPageRank::default();
    let (ranked, subgraph) = graph_search(
        &*backend,
        &*backend,
        &ns,
        std::slice::from_ref(&alice.id),
        2,
        10,
        &TemporalFilter::default(),
        &ppr,
    )
    .await
    .unwrap();
    assert_eq!(ranked.len(), 2);
    assert_eq!(ranked[0].item.id, m_near.id);

    let entity_ranks = ppr.rank(&subgraph, &[alice.id]);
    let scores = graph_scores(&subgraph, &entity_ranks);
    assert!(scores[&m_near.id] > scores[&m_far.id]);
}

#[test]
fn rrf_fuses_multiple_rankings_deterministically() {
    let ns = test_ns();
    let a = make_item(&ns, "a", vec![1.0, 0.0]);
    let b = make_item(&ns, "b", vec![0.0, 1.0]);
    let c = make_item(&ns, "c", vec![0.5, 0.5]);

    let list1 = vec![
        Candidate {
            item: a.clone(),
            score: 0.9,
        },
        Candidate {
            item: b.clone(),
            score: 0.5,
        },
    ];
    let list2 = vec![
        Candidate {
            item: b.clone(),
            score: 10.0,
        },
        Candidate {
            item: c.clone(),
            score: 5.0,
        },
    ];

    let fused = rrf(&[list1, list2], RRF_K);
    assert_eq!(fused[0].item.id, b.id);
    assert!(fused[0].score > fused[1].score);
    assert_eq!(fused.len(), 3);
}

#[test]
fn scope_filter_and_in_scopes_enforce_user_isolation() {
    let ns = test_ns();
    let u1 = UserId::new("u1").unwrap();
    let u2 = UserId::new("u2").unwrap();

    let mut in_u1 = MemoryInput::new(ns.clone(), "u1 secret");
    in_u1.scope = MemoryScope::User(u1.clone());
    let item_u1 = MemoryItem::from_input(in_u1, 1);

    let mut in_u2 = MemoryInput::new(ns.clone(), "u2 secret");
    in_u2.scope = MemoryScope::User(u2);
    let item_u2 = MemoryItem::from_input(in_u2, 1);

    let proj_item = make_item(&ns, "project shared", vec![1.0]);

    let requested = vec![MemoryScope::Project, MemoryScope::User(u1)];
    let filter = scope_filter(&requested).unwrap();
    assert!(filter.matches(&index_metadata_like(&item_u1)));
    assert!(!filter.matches(&index_metadata_like(&item_u2)));
    assert!(filter.matches(&index_metadata_like(&proj_item)));

    let filtered = in_scopes(
        vec![
            Candidate {
                item: item_u1.clone(),
                score: 0.9,
            },
            Candidate {
                item: item_u2,
                score: 0.95,
            },
            Candidate {
                item: proj_item.clone(),
                score: 0.8,
            },
        ],
        &requested,
    );
    assert_eq!(filtered.len(), 2);
    assert!(filtered
        .iter()
        .all(|c| c.item.id == item_u1.id || c.item.id == proj_item.id));
}

fn index_metadata_like(item: &MemoryItem) -> serde_json::Value {
    muninn_aether_cortex::indexer::index_metadata(item)
}
