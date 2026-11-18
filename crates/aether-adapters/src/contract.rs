//! Reusable backend contract tests for `RecordStore`, `VectorStore`, and `GraphStore`.
//!
//! Each suite uses freshly generated [`ProjectId`] namespaces so real shared
//! backends can run them concurrently and repeatedly without cross-test interference.

use std::sync::Arc;
use std::time::Duration;

use aether_core::{
    AetherError, CreateOutcome, DistanceMetric, Entity, EntityId, FilterExpr, FilterValue,
    GraphStore, IndexSpec, MemoryId, MemoryInput, MemoryItem, MemoryReceipt, MemoryScope,
    PendingRecord, ProjectId, ReceiptState, RecordStore, Relation, SessionId, TemporalFilter,
    VectorRecord, VectorStore,
};
use chrono::{Duration as ChronoDuration, Utc};
use serde_json::json;
use uuid::Uuid;

fn fresh_project(prefix: &str) -> ProjectId {
    ProjectId::new(format!("{prefix}-{}", Uuid::new_v4())).expect("valid generated project id")
}

/// Verifies `RecordStore` contract: `create`, idempotency, namespace isolation, CAS conflict, `pending`, `mark_indexed`, `get_many`, `session_items`, and `tombstone`.
pub async fn record_store<S: RecordStore>(store: &S) {
    let ns_a = fresh_project("recv-a");
    let ns_b = fresh_project("recv-b");

    // Versioned records: create, namespace isolation, pending/pending_for, mark_indexed, get_many, tombstone.
    let mut v_input = MemoryInput::new(ns_a.clone(), "Versioned record");
    v_input.idempotency_key = Some("v-idem-1".into());
    let v_item = MemoryItem::from_input(v_input, 1);

    let out1 = store.create(&ns_a, &v_item, None).await.unwrap();
    assert_eq!(out1, CreateOutcome::Created);
    assert!(store.get(&ns_b, v_item.id).await.unwrap().is_none());

    // Same idempotency key in a different namespace is NOT a duplicate.
    let mut ns_b_input = MemoryInput::new(ns_b.clone(), "Other tenant");
    ns_b_input.idempotency_key = Some("v-idem-1".into());
    let ns_b_item = MemoryItem::from_input(ns_b_input, 1);
    assert_eq!(
        store.create(&ns_b, &ns_b_item, None).await.unwrap(),
        CreateOutcome::Created
    );
    store.mark_indexed(&ns_b, ns_b_item.id, 1).await.unwrap();

    // Duplicate idempotency key returns Duplicate(Buffered) before indexing.
    let mut dup_input = MemoryInput::new(ns_a.clone(), "Different body");
    dup_input.idempotency_key = Some("v-idem-1".into());
    let dup_item = MemoryItem::from_input(dup_input, 1);
    let out2 = store.create(&ns_a, &dup_item, None).await.unwrap();
    assert_eq!(
        out2,
        CreateOutcome::Duplicate(MemoryReceipt {
            memory_id: v_item.id,
            state: ReceiptState::Buffered,
            version: 1,
        })
    );

    // Retried create for same MemoryId without idempotency key succeeds as Created and stores 1 record.
    let mut retry_item = v_item.clone();
    retry_item.idempotency_key = None;
    assert_eq!(
        store.create(&ns_a, &retry_item, None).await.unwrap(),
        CreateOutcome::Created
    );
    assert_eq!(store.get_many(&ns_a, &[v_item.id]).await.unwrap().len(), 1);

    let pf = store.pending_for(&ns_a, 100).await.unwrap();
    assert!(pf.iter().any(|x| x.id == v_item.id));
    let all_p = store.pending(1000).await.unwrap();
    assert!(all_p
        .iter()
        .any(|p: &PendingRecord| p.item.id == v_item.id && !p.tombstone));

    // Stale mark_indexed is a no-op; matching mark_indexed transitions to Persisted.
    store.mark_indexed(&ns_a, v_item.id, 99).await.unwrap();
    assert!(store
        .pending_for(&ns_a, 100)
        .await
        .unwrap()
        .iter()
        .any(|x| x.id == v_item.id));
    store.mark_indexed(&ns_a, v_item.id, 1).await.unwrap();
    assert!(!store
        .pending_for(&ns_a, 100)
        .await
        .unwrap()
        .iter()
        .any(|x| x.id == v_item.id));

    let out_after_idx = store.create(&ns_a, &dup_item, None).await.unwrap();
    assert_eq!(
        out_after_idx,
        CreateOutcome::Duplicate(MemoryReceipt {
            memory_id: v_item.id,
            state: ReceiptState::Persisted,
            version: 1,
        })
    );

    // CAS update puts the record back in pending_for; stale expected_version conflicts.
    let mut v_item_2 = v_item.clone();
    v_item_2.content = "Versioned record v2".into();
    v_item_2.version = 2;
    store
        .cas_update(&ns_a, v_item.id, Some(1), &v_item_2)
        .await
        .unwrap();
    let stale_conflict = store.cas_update(&ns_a, v_item.id, Some(1), &v_item_2).await;
    assert!(matches!(stale_conflict, Err(AetherError::Conflict { .. })));
    assert!(store
        .pending_for(&ns_a, 100)
        .await
        .unwrap()
        .iter()
        .any(|x| x.id == v_item.id && x.version == 2));
    store.mark_indexed(&ns_a, v_item.id, 2).await.unwrap();

    // Tombstone bumps version, hides from get/get_many/pending_for, releases idempotency key,
    // stays in pending(tombstone: true) until mark_indexed removes it.
    let tomb = store.tombstone(&ns_a, v_item.id).await.unwrap().unwrap();
    assert_eq!(tomb.version, 3);
    assert!(store.get(&ns_a, v_item.id).await.unwrap().is_none());
    assert!(store
        .get_many(&ns_a, &[v_item.id])
        .await
        .unwrap()
        .is_empty());
    assert!(!store
        .pending_for(&ns_a, 100)
        .await
        .unwrap()
        .iter()
        .any(|x| x.id == v_item.id));
    assert!(store
        .pending(1000)
        .await
        .unwrap()
        .iter()
        .any(|p| p.item.id == v_item.id && p.tombstone && p.item.version == 3));
    store.mark_indexed(&ns_a, v_item.id, 3).await.unwrap();
    assert!(!store
        .pending(1000)
        .await
        .unwrap()
        .iter()
        .any(|p| p.item.id == v_item.id));
    assert!(store.tombstone(&ns_a, v_item.id).await.unwrap().is_none());

    // Released idempotency key can now be reused.
    let mut reuse_input = MemoryInput::new(ns_a.clone(), "Reused key");
    reuse_input.idempotency_key = Some("v-idem-1".into());
    let reuse_item = MemoryItem::from_input(reuse_input, 1);
    assert_eq!(
        store.create(&ns_a, &reuse_item, None).await.unwrap(),
        CreateOutcome::Created
    );

    // Expired records from create are invisible to get and pending.
    let exp_item = MemoryItem::from_input(MemoryInput::new(ns_a.clone(), "Short TTL"), 1);
    store
        .create(&ns_a, &exp_item, Some(Duration::from_millis(20)))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(35)).await;
    assert!(store.get(&ns_a, exp_item.id).await.unwrap().is_none());
    assert!(!store
        .pending(1000)
        .await
        .unwrap()
        .iter()
        .any(|p| p.item.id == exp_item.id));

    // session_items returns only live, unexpired, non-tombstoned items for that session.
    let s1 = SessionId::new("s-1").unwrap();
    let s2 = SessionId::new("s-2").unwrap();
    let mut s1_live_in = MemoryInput::new(ns_a.clone(), "s1 live");
    s1_live_in.scope = MemoryScope::Session(s1.clone());
    let s1_live = MemoryItem::from_input(s1_live_in, 1);

    let mut s1_exp_in = MemoryInput::new(ns_a.clone(), "s1 expired");
    s1_exp_in.scope = MemoryScope::Session(s1.clone());
    let s1_exp = MemoryItem::from_input(s1_exp_in, 1);

    let mut s1_tomb_in = MemoryInput::new(ns_a.clone(), "s1 tombstoned");
    s1_tomb_in.scope = MemoryScope::Session(s1.clone());
    let s1_tomb = MemoryItem::from_input(s1_tomb_in, 1);

    let mut s2_in = MemoryInput::new(ns_a.clone(), "s2 live");
    s2_in.scope = MemoryScope::Session(s2.clone());
    let s2_live = MemoryItem::from_input(s2_in, 1);

    let proj_only = MemoryItem::from_input(MemoryInput::new(ns_a.clone(), "proj"), 1);

    store.create(&ns_a, &s1_live, None).await.unwrap();
    store
        .create(&ns_a, &s1_exp, Some(Duration::from_millis(20)))
        .await
        .unwrap();
    store.create(&ns_a, &s1_tomb, None).await.unwrap();
    store.create(&ns_a, &s2_live, None).await.unwrap();
    store.create(&ns_a, &proj_only, None).await.unwrap();
    store.tombstone(&ns_a, s1_tomb.id).await.unwrap();
    tokio::time::sleep(Duration::from_millis(35)).await;

    let s1_items = store.session_items(&ns_a, &s1, 10).await.unwrap();
    assert_eq!(s1_items.len(), 1);
    assert_eq!(s1_items[0].id, s1_live.id);
}

/// Verifies concurrent `create` calls with one idempotency key produce exactly one record.
pub async fn record_store_concurrent_idempotency<S: RecordStore + 'static>(store: Arc<S>) {
    let ns = fresh_project("rec-conc");
    let mut handles = Vec::with_capacity(16);
    for idx in 0..16 {
        let store = Arc::clone(&store);
        let ns = ns.clone();
        handles.push(tokio::spawn(async move {
            let mut input = MemoryInput::new(ns.clone(), format!("Concurrent {idx}"));
            input.idempotency_key = Some("shared-conc-key".into());
            let item = MemoryItem::from_input(input, 1);
            let out = store.create(&ns, &item, None).await.unwrap();
            (item.id, out)
        }));
    }

    let mut created_count = 0usize;
    let mut resolved_ids = Vec::with_capacity(16);
    for handle in handles {
        let (local_id, outcome) = handle.await.unwrap();
        match outcome {
            CreateOutcome::Created => {
                created_count += 1;
                resolved_ids.push(local_id);
            }
            CreateOutcome::Duplicate(receipt) => resolved_ids.push(receipt.memory_id),
        }
    }
    assert_eq!(created_count, 1);
    let first = resolved_ids[0];
    assert!(resolved_ids.iter().all(|id| *id == first));
}

/// Verifies `VectorStore` contract: namespace isolation, upsert/delete/dimension checks, and filtering.
pub async fn vector_store<S: VectorStore>(store: &S) {
    vector_namespace_isolation(store).await;
    vector_upsert_delete_and_dimension_check(store).await;
    vector_filter_expr_evaluation(store).await;
}

async fn vector_namespace_isolation<S: VectorStore>(store: &S) {
    let ns_a = fresh_project("vec-a");
    let ns_b = fresh_project("vec-b");

    assert_eq!(store.describe(&ns_a).await.unwrap(), None);

    store
        .initialize(&ns_a, 3, DistanceMetric::Cosine)
        .await
        .unwrap();
    assert_eq!(
        store.describe(&ns_a).await.unwrap(),
        Some(IndexSpec {
            dimensions: 3,
            metric: DistanceMetric::Cosine
        })
    );
    store
        .initialize(&ns_b, 3, DistanceMetric::Cosine)
        .await
        .unwrap();

    let mem_a = MemoryId::new();
    store
        .upsert_batch(
            &ns_a,
            &[VectorRecord {
                id: mem_a,
                vector: vec![1.0, 0.0, 0.0],
                metadata: json!({"tier": "semantic"}),
            }],
        )
        .await
        .unwrap();

    let hits_a = store
        .search(&ns_a, &[1.0, 0.0, 0.0], 5, None)
        .await
        .unwrap();
    let hits_b = store
        .search(&ns_b, &[1.0, 0.0, 0.0], 5, None)
        .await
        .unwrap();
    assert_eq!(hits_a.len(), 1);
    assert!(hits_b.is_empty());
}

async fn vector_upsert_delete_and_dimension_check<S: VectorStore>(store: &S) {
    let ns = fresh_project("proj-vec");
    store
        .initialize(&ns, 3, DistanceMetric::Cosine)
        .await
        .unwrap();

    let id1 = MemoryId::new();
    let id2 = MemoryId::new();
    store
        .upsert_batch(
            &ns,
            &[
                VectorRecord {
                    id: id1,
                    vector: vec![1.0, 0.0, 0.0],
                    metadata: json!({"tier": "semantic", "confidence": 0.9}),
                },
                VectorRecord {
                    id: id2,
                    vector: vec![0.0, 1.0, 0.0],
                    metadata: json!({"tier": "episodic", "confidence": 0.4}),
                },
            ],
        )
        .await
        .unwrap();

    let dim_err = store
        .upsert_batch(
            &ns,
            &[VectorRecord {
                id: MemoryId::new(),
                vector: vec![1.0, 0.0],
                metadata: json!({}),
            }],
        )
        .await;
    assert!(dim_err.is_err());

    let matches = store.search(&ns, &[1.0, 0.0, 0.0], 2, None).await.unwrap();
    assert_eq!(matches[0].id, id1);

    store.delete(&ns, &[id1]).await.unwrap();
    store.delete(&ns, &[id1]).await.unwrap();
    let after_del = store.search(&ns, &[1.0, 0.0, 0.0], 2, None).await.unwrap();
    assert_eq!(after_del.len(), 1);
    assert_eq!(after_del[0].id, id2);
}

async fn vector_filter_expr_evaluation<S: VectorStore>(store: &S) {
    let ns = fresh_project("proj-filter");
    store.initialize(&ns, 2, DistanceMetric::Dot).await.unwrap();

    let id1 = MemoryId::new();
    let id2 = MemoryId::new();
    let id3 = MemoryId::new();
    store
        .upsert_batch(
            &ns,
            &[
                VectorRecord {
                    id: id1,
                    vector: vec![1.0, 0.0],
                    metadata: json!({"tier": "semantic", "score": 0.9}),
                },
                VectorRecord {
                    id: id2,
                    vector: vec![0.9, 0.1],
                    metadata: json!({"tier": "episodic", "score": 0.7}),
                },
                VectorRecord {
                    id: id3,
                    vector: vec![0.8, 0.2],
                    metadata: json!({"tier": "semantic", "score": 0.3}),
                },
            ],
        )
        .await
        .unwrap();

    let filter = FilterExpr::And(vec![
        FilterExpr::In {
            field: "tier".into(),
            values: vec![FilterValue::String("semantic".into())],
        },
        FilterExpr::Range {
            field: "score".into(),
            gte: Some(0.5),
            lte: None,
        },
    ]);

    let hits = store
        .search(&ns, &[1.0, 0.0], 10, Some(&filter))
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, id1);
}

/// Verifies `GraphStore` contract: namespace isolation, temporal provenance, and fact retraction.
pub async fn graph_store<S: GraphStore>(store: &S) {
    graph_namespace_isolation(store).await;
    graph_temporal_provenance_and_retract(store).await;
}

async fn graph_namespace_isolation<S: GraphStore>(store: &S) {
    let ns_a = fresh_project("graph-a");
    let ns_b = fresh_project("graph-b");

    let alice = Entity {
        id: EntityId::new("person:alice").unwrap(),
        name: "Alice".into(),
        entity_type: "Person".into(),
    };
    let qdrant = Entity {
        id: EntityId::new("db:qdrant").unwrap(),
        name: "Qdrant".into(),
        entity_type: "Database".into(),
    };
    store
        .upsert_entities(&ns_a, &[alice.clone(), qdrant.clone()])
        .await
        .unwrap();
    store
        .upsert_relations(
            &ns_a,
            MemoryId::new(),
            &[Relation {
                source: alice.id.clone(),
                target: qdrant.id.clone(),
                relation_type: "USES".into(),
                valid_at: Utc::now(),
                invalid_at: None,
            }],
        )
        .await
        .unwrap();

    let sub_a = store
        .neighborhood(
            &ns_a,
            std::slice::from_ref(&alice.id),
            1,
            10,
            &TemporalFilter::default(),
        )
        .await
        .unwrap();
    let sub_b = store
        .neighborhood(&ns_b, &[alice.id], 1, 10, &TemporalFilter::default())
        .await
        .unwrap();
    assert_eq!(sub_a.edges.len(), 1);
    assert!(sub_b.edges.is_empty());
}

async fn graph_temporal_provenance_and_retract<S: GraphStore>(store: &S) {
    let ns = fresh_project("proj-graph");

    let alice = Entity {
        id: EntityId::new("person:alice").unwrap(),
        name: "Alice".into(),
        entity_type: "Person".into(),
    };
    let neo4j = Entity {
        id: EntityId::new("db:neo4j").unwrap(),
        name: "Neo4j".into(),
        entity_type: "Database".into(),
    };
    let chroma = Entity {
        id: EntityId::new("db:chroma").unwrap(),
        name: "Chroma".into(),
        entity_type: "Database".into(),
    };
    store
        .upsert_entities(&ns, &[alice.clone(), neo4j.clone(), chroma.clone()])
        .await
        .unwrap();

    let t0 = Utc::now() - ChronoDuration::minutes(10);
    let t_retract = Utc::now() - ChronoDuration::minutes(2);
    let mem1 = MemoryId::new();
    let mem2 = MemoryId::new();

    store
        .upsert_relations(
            &ns,
            mem1,
            &[Relation {
                source: alice.id.clone(),
                target: chroma.id.clone(),
                relation_type: "USED".into(),
                valid_at: t0,
                invalid_at: None,
            }],
        )
        .await
        .unwrap();

    store
        .upsert_relations(
            &ns,
            mem2,
            &[Relation {
                source: alice.id.clone(),
                target: neo4j.id.clone(),
                relation_type: "USES".into(),
                valid_at: t0,
                invalid_at: None,
            }],
        )
        .await
        .unwrap();

    store
        .retract_memory_facts(&ns, mem1, t_retract)
        .await
        .unwrap();

    let active = store
        .neighborhood(
            &ns,
            std::slice::from_ref(&alice.id),
            1,
            10,
            &TemporalFilter::default(),
        )
        .await
        .unwrap();
    assert_eq!(active.edges.len(), 1);
    assert_eq!(active.edges[0].source_memory_id, mem2);

    let historical = store
        .neighborhood(
            &ns,
            std::slice::from_ref(&alice.id),
            1,
            10,
            &TemporalFilter {
                as_of: Some(t0 + ChronoDuration::minutes(1)),
                active_only: false,
            },
        )
        .await
        .unwrap();
    assert_eq!(historical.edges.len(), 2);

    // Idempotent relation upserts: repeating or re-upserting after retraction replaces in place.
    let mem3 = MemoryId::new();
    let rel3 = Relation {
        source: alice.id.clone(),
        target: neo4j.id.clone(),
        relation_type: "PREFERS".into(),
        valid_at: t0,
        invalid_at: None,
    };
    store
        .upsert_relations(&ns, mem3, std::slice::from_ref(&rel3))
        .await
        .unwrap();
    store
        .upsert_relations(&ns, mem3, std::slice::from_ref(&rel3))
        .await
        .unwrap();
    let total_for_mem3 = store
        .neighborhood(
            &ns,
            std::slice::from_ref(&alice.id),
            1,
            25,
            &TemporalFilter {
                as_of: None,
                active_only: false,
            },
        )
        .await
        .unwrap()
        .edges
        .into_iter()
        .filter(|e| e.source_memory_id == mem3)
        .count();
    assert_eq!(total_for_mem3, 1);

    store
        .retract_memory_facts(&ns, mem3, t_retract)
        .await
        .unwrap();
    let active_after_retract = store
        .neighborhood(
            &ns,
            std::slice::from_ref(&alice.id),
            1,
            25,
            &TemporalFilter::default(),
        )
        .await
        .unwrap()
        .edges
        .into_iter()
        .filter(|e| e.source_memory_id == mem3)
        .count();
    assert_eq!(active_after_retract, 0);

    store
        .upsert_relations(&ns, mem3, std::slice::from_ref(&rel3))
        .await
        .unwrap();
    let active_after_reupsert = store
        .neighborhood(
            &ns,
            std::slice::from_ref(&alice.id),
            1,
            25,
            &TemporalFilter::default(),
        )
        .await
        .unwrap()
        .edges
        .into_iter()
        .filter(|e| e.source_memory_id == mem3)
        .count();
    assert_eq!(active_after_reupsert, 1);
    let total_after_reupsert = store
        .neighborhood(
            &ns,
            &[alice.id],
            1,
            25,
            &TemporalFilter {
                as_of: None,
                active_only: false,
            },
        )
        .await
        .unwrap()
        .edges
        .into_iter()
        .filter(|e| e.source_memory_id == mem3)
        .count();
    assert_eq!(total_after_reupsert, 1);
}
