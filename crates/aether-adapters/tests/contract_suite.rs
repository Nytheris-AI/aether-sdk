use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use aether_adapters::memory_test::InMemoryBackend;
use aether_adapters::middleware::{Batcher, Resilient, RetryPolicy, Timeout, Traced};
use aether_core::{
    AetherError, ConnectorCapabilities, ConnectorHealth, DistanceMetric, Entity, EntityId,
    FilterExpr, FilterValue, GraphStore, KvStore, MemoryId, MemoryInput, MemoryItem,
    MemoryReceipt, MemoryScope, OutboundConnector, ProjectId, ReceiptState, Relation, Result,
    Subgraph, TemporalFilter, VectorMatch, VectorRecord, VectorStore,
};
use async_trait::async_trait;
use chrono::{Duration as ChronoDuration, Utc};
use serde_json::json;

#[tokio::test]
async fn contract_namespace_isolation_vector_and_graph() {
    let backend = InMemoryBackend::new();
    let ns_a = ProjectId::new("tenant-a").unwrap();
    let ns_b = ProjectId::new("tenant-b").unwrap();

    backend.initialize(&ns_a, 3, DistanceMetric::Cosine).await.unwrap();
    backend.initialize(&ns_b, 3, DistanceMetric::Cosine).await.unwrap();

    let mem_a = MemoryId::new();
    backend
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

    let hits_a = backend.search(&ns_a, &[1.0, 0.0, 0.0], 5, None).await.unwrap();
    let hits_b = backend.search(&ns_b, &[1.0, 0.0, 0.0], 5, None).await.unwrap();
    assert_eq!(hits_a.len(), 1);
    assert!(hits_b.is_empty());

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
    backend.upsert_entities(&ns_a, &[alice.clone(), qdrant.clone()]).await.unwrap();
    backend
        .upsert_relations(
            &ns_a,
            mem_a,
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

    let sub_a = backend
        .neighborhood(&ns_a, &[alice.id.clone()], 1, 10, &TemporalFilter::default())
        .await
        .unwrap();
    let sub_b = backend
        .neighborhood(&ns_b, &[alice.id], 1, 10, &TemporalFilter::default())
        .await
        .unwrap();
    assert_eq!(sub_a.edges.len(), 1);
    assert!(sub_b.edges.is_empty());
}

#[tokio::test]
async fn contract_upsert_delete_and_dimension_check() {
    let backend = InMemoryBackend::new();
    let ns = ProjectId::new("proj-vec").unwrap();
    backend.initialize(&ns, 3, DistanceMetric::Cosine).await.unwrap();

    let id1 = MemoryId::new();
    let id2 = MemoryId::new();
    backend
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

    let dim_err = backend
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

    let matches = backend.search(&ns, &[1.0, 0.0, 0.0], 2, None).await.unwrap();
    assert_eq!(matches[0].id, id1);

    VectorStore::delete(&backend, &ns, &[id1]).await.unwrap();
    VectorStore::delete(&backend, &ns, &[id1]).await.unwrap();
    let after_del = backend.search(&ns, &[1.0, 0.0, 0.0], 2, None).await.unwrap();
    assert_eq!(after_del.len(), 1);
    assert_eq!(after_del[0].id, id2);
}

#[tokio::test]
async fn contract_filter_expr_evaluation() {
    let backend = InMemoryBackend::new();
    let ns = ProjectId::new("proj-filter").unwrap();
    backend.initialize(&ns, 2, DistanceMetric::Dot).await.unwrap();

    let id1 = MemoryId::new();
    let id2 = MemoryId::new();
    let id3 = MemoryId::new();
    backend
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

    let hits = backend.search(&ns, &[1.0, 0.0], 10, Some(&filter)).await.unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, id1);
}

#[tokio::test]
async fn contract_temporal_provenance_and_retract_memory_facts() {
    let backend = InMemoryBackend::new();
    let ns = ProjectId::new("proj-graph").unwrap();

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
    backend.upsert_entities(&ns, &[alice.clone(), neo4j.clone(), chroma.clone()]).await.unwrap();

    let t0 = Utc::now() - ChronoDuration::minutes(10);
    let t_retract = Utc::now() - ChronoDuration::minutes(2);
    let mem1 = MemoryId::new();
    let mem2 = MemoryId::new();

    backend
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

    backend
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

    backend.retract_memory_facts(&ns, mem1, t_retract).await.unwrap();

    let active = backend
        .neighborhood(&ns, &[alice.id.clone()], 1, 10, &TemporalFilter::default())
        .await
        .unwrap();
    assert_eq!(active.edges.len(), 1);
    assert_eq!(active.edges[0].source_memory_id, mem2);

    let historical = backend
        .neighborhood(
            &ns,
            &[alice.id],
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
}

struct FlakyStore {
    attempts: AtomicUsize,
    fail_times: usize,
    permanent: bool,
    batch_sizes: std::sync::Mutex<Vec<usize>>,
}

#[async_trait]
impl OutboundConnector for FlakyStore {
    fn name(&self) -> &'static str {
        "flaky"
    }
    fn capabilities(&self) -> ConnectorCapabilities {
        ConnectorCapabilities {
            max_batch_size: 2,
            supports_temporal: false,
            supports_filter_expr: false,
        }
    }
    async fn health_check(&self) -> Result<ConnectorHealth> {
        Ok(ConnectorHealth::Healthy)
    }
    async fn close(&self) -> Result<()> {
        Ok(())
    }
}

#[async_trait]
impl VectorStore for FlakyStore {
    async fn initialize(&self, _ns: &ProjectId, _d: usize, _m: DistanceMetric) -> Result<()> {
        Ok(())
    }
    async fn upsert_batch(&self, _ns: &ProjectId, records: &[VectorRecord]) -> Result<()> {
        self.batch_sizes.lock().unwrap().push(records.len());
        let n = self.attempts.fetch_add(1, Ordering::SeqCst);
        if n < self.fail_times {
            return if self.permanent {
                Err(AetherError::Permanent {
                    connector: "flaky",
                    message: "bad api key".into(),
                })
            } else {
                Err(AetherError::Transient {
                    connector: "flaky",
                    message: "429 rate limited".into(),
                })
            };
        }
        Ok(())
    }
    async fn delete(&self, _ns: &ProjectId, _ids: &[MemoryId]) -> Result<()> {
        Ok(())
    }
    async fn search(
        &self,
        _ns: &ProjectId,
        _q: &[f32],
        _k: usize,
        _f: Option<&FilterExpr>,
    ) -> Result<Vec<VectorMatch>> {
        Ok(vec![])
    }
}

#[tokio::test]
async fn contract_resilient_retries_transient_only_and_batcher_chunks() {
    let store = Arc::new(FlakyStore {
        attempts: AtomicUsize::new(0),
        fail_times: 2,
        permanent: false,
        batch_sizes: Default::default(),
    });
    let wrapped = Traced::new(Resilient::new(
        Timeout::new(Batcher::new(store.clone()), Duration::from_secs(2)),
        RetryPolicy {
            max_retries: 3,
            base_delay: Duration::from_millis(1),
        },
    ));

    let ns = ProjectId::new("proj-mw").unwrap();
    wrapped
        .upsert_batch(
            &ns,
            &[
                VectorRecord {
                    id: MemoryId::new(),
                    vector: vec![1.0],
                    metadata: json!({}),
                },
                VectorRecord {
                    id: MemoryId::new(),
                    vector: vec![1.0],
                    metadata: json!({}),
                },
                VectorRecord {
                    id: MemoryId::new(),
                    vector: vec![1.0],
                    metadata: json!({}),
                },
            ],
        )
        .await
        .unwrap();

    // First chunk of 2 failed twice (attempts 0, 1) then succeeded (attempt 2); second chunk of 1 succeeded (attempt 3).
    assert_eq!(store.attempts.load(Ordering::SeqCst), 4);
    assert_eq!(*store.batch_sizes.lock().unwrap(), vec![2, 2, 2, 1]);

    // Permanent errors must fail immediately without retry.
    let perm_store = Arc::new(FlakyStore {
        attempts: AtomicUsize::new(0),
        fail_times: 5,
        permanent: true,
        batch_sizes: Default::default(),
    });
    let perm_wrapped = Resilient::new(
        perm_store.clone(),
        RetryPolicy {
            max_retries: 3,
            base_delay: Duration::from_millis(1),
        },
    );
    let res = perm_wrapped
        .upsert_batch(
            &ns,
            &[VectorRecord {
                id: MemoryId::new(),
                vector: vec![1.0],
                metadata: json!({}),
            }],
        )
        .await;
    assert!(res.is_err());
    assert_eq!(perm_store.attempts.load(Ordering::SeqCst), 1);
    let _sub = Subgraph::default();
}

#[tokio::test]
async fn contract_kv_store_ttl_idempotency_cas_and_recent() {
    let backend = Arc::new(InMemoryBackend::new());
    let kv: Arc<dyn KvStore> = Arc::new(Traced::new(Resilient::new(
        Timeout::new(backend, Duration::from_secs(2)),
        RetryPolicy::default(),
    )));

    let ns_a = ProjectId::new("kv-a").unwrap();
    let ns_b = ProjectId::new("kv-b").unwrap();

    let mut input1 = MemoryInput::new(ns_a.clone(), "Session turn 1");
    input1.scope = MemoryScope::Session;
    let item1 = MemoryItem::from_input(input1, 1);

    // 1. Put with TTL and verify expiration.
    kv.put(&ns_a, &item1, Some(Duration::from_millis(25)))
        .await
        .unwrap();
    assert!(kv.get(&ns_a, item1.id).await.unwrap().is_some());
    assert!(kv.get(&ns_b, item1.id).await.unwrap().is_none());

    tokio::time::sleep(Duration::from_millis(40)).await;
    assert!(kv.get(&ns_a, item1.id).await.unwrap().is_none());

    // 2. Atomic idempotency claim (put_if_absent).
    let receipt = MemoryReceipt {
        memory_id: item1.id,
        state: ReceiptState::Buffered,
        version: 1,
    };
    assert!(kv
        .put_if_absent(&ns_a, "idem-1", &receipt, None)
        .await
        .unwrap()
        .is_none());
    let dup = kv
        .put_if_absent(&ns_a, "idem-1", &receipt, None)
        .await
        .unwrap();
    assert_eq!(dup, Some(receipt));
    assert!(kv
        .put_if_absent(&ns_b, "idem-1", &MemoryReceipt {
            memory_id: MemoryId::new(),
            state: ReceiptState::Buffered,
            version: 1,
        }, None)
        .await
        .unwrap()
        .is_none());

    // 3. CAS update + Conflict on stale expected_version.
    kv.put(&ns_a, &item1, None).await.unwrap();
    let mut v2 = item1.clone();
    v2.content = "Session turn 1 updated".into();
    v2.version = 2;
    kv.cas_update(&ns_a, item1.id, Some(1), &v2).await.unwrap();

    let mut stale = item1.clone();
    stale.version = 2;
    let conflict = kv.cas_update(&ns_a, item1.id, Some(1), &stale).await;
    assert!(matches!(conflict, Err(AetherError::Conflict { .. })));

    // 4. Recent sliding window & scope filter.
    let mut proj_input = MemoryInput::new(ns_a.clone(), "Project fact");
    proj_input.scope = MemoryScope::Project;
    let proj_item = MemoryItem::from_input(proj_input, 1);
    kv.put(&ns_a, &proj_item, None).await.unwrap();

    let session_recent = kv.recent(&ns_a, Some(MemoryScope::Session), 10).await.unwrap();
    assert_eq!(session_recent.len(), 1);
    assert_eq!(session_recent[0].id, item1.id);

    let all_recent = kv.recent(&ns_a, None, 10).await.unwrap();
    assert_eq!(all_recent.len(), 2);

    kv.delete(&ns_a, item1.id).await.unwrap();
    assert!(kv.get(&ns_a, item1.id).await.unwrap().is_none());
}
