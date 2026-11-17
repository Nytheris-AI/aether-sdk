use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use aether_adapters::contract;
use aether_adapters::middleware::{jittered, Batcher, Resilient, RetryPolicy, Timeout, Traced};
use aether_adapters::{FaultPoint, InMemoryBackend};
use aether_core::{
    AetherError, ConnectorCapabilities, ConnectorHealth, DistanceMetric, Entity, EntityId,
    FilterExpr, GraphStore, IndexSpec, MemoryId, OutboundConnector, ProjectId, Relation, Result,
    TemporalFilter, VectorMatch, VectorRecord, VectorStore,
};
use async_trait::async_trait;
use chrono::Utc;
use serde_json::json;

#[tokio::test]
async fn contract_vector_store_in_memory() {
    let backend = InMemoryBackend::new();
    contract::vector_store(&backend).await;
}

#[tokio::test]
async fn contract_graph_store_in_memory() {
    let backend = InMemoryBackend::new();
    contract::graph_store(&backend).await;
}

#[tokio::test]
async fn contract_record_store_in_memory_through_middleware() {
    let backend = Arc::new(InMemoryBackend::new());
    let store = Traced::new(Resilient::new(
        Timeout::new(backend, Duration::from_secs(2)),
        RetryPolicy::default(),
    ));
    contract::record_store(&store).await;
    contract::record_store_versioned(&store).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn contract_record_store_concurrent_idempotency_in_memory() {
    let backend = Arc::new(InMemoryBackend::new());
    contract::record_store_concurrent_idempotency(backend).await;
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
    async fn describe(&self, _ns: &ProjectId) -> Result<Option<IndexSpec>> {
        Ok(None)
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
}

#[tokio::test]
async fn in_memory_backend_injects_transient_faults_then_recovers() {
    let b = InMemoryBackend::new();
    let p = ProjectId::new(format!("t-{}", uuid::Uuid::new_v4())).unwrap();
    b.initialize(&p, 2, DistanceMetric::Cosine).await.unwrap();
    let rec = VectorRecord {
        id: MemoryId::new(),
        vector: vec![1.0, 0.0],
        metadata: json!({}),
    };
    b.fail_next(FaultPoint::VectorUpsert, 1);
    let err = b
        .upsert_batch(&p, std::slice::from_ref(&rec))
        .await
        .unwrap_err();
    assert!(err.is_retryable());
    assert!(!b.has_vector(&p, rec.id));
    b.upsert_batch(&p, std::slice::from_ref(&rec))
        .await
        .unwrap();
    assert!(b.has_vector(&p, rec.id));
}

#[tokio::test]
async fn in_memory_backend_injects_delete_and_graph_faults() {
    let b = InMemoryBackend::new();
    let p = ProjectId::new(format!("t-{}", uuid::Uuid::new_v4())).unwrap();
    b.initialize(&p, 2, DistanceMetric::Cosine).await.unwrap();
    let rec = VectorRecord {
        id: MemoryId::new(),
        vector: vec![1.0, 0.0],
        metadata: json!({}),
    };
    b.upsert_batch(&p, std::slice::from_ref(&rec))
        .await
        .unwrap();

    b.fail_next(FaultPoint::VectorDelete, 1);
    let err = VectorStore::delete(&b, &p, &[rec.id]).await.unwrap_err();
    assert!(err.is_retryable());
    assert!(b.has_vector(&p, rec.id));
    VectorStore::delete(&b, &p, &[rec.id]).await.unwrap();
    assert!(!b.has_vector(&p, rec.id));

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
    b.fail_next(FaultPoint::GraphUpsert, 2);
    assert!(b
        .upsert_entities(&p, std::slice::from_ref(&alice))
        .await
        .unwrap_err()
        .is_retryable());
    let mem_id = MemoryId::new();
    let rel = Relation {
        source: alice.id.clone(),
        target: bob.id.clone(),
        relation_type: "KNOWS".into(),
        valid_at: Utc::now(),
        invalid_at: None,
    };
    assert!(b
        .upsert_relations(&p, mem_id, std::slice::from_ref(&rel))
        .await
        .unwrap_err()
        .is_retryable());
    b.upsert_entities(&p, &[alice.clone(), bob.clone()])
        .await
        .unwrap();
    b.upsert_relations(&p, mem_id, &[rel]).await.unwrap();

    b.fail_next(FaultPoint::GraphRetract, 1);
    assert!(b
        .retract_memory_facts(&p, mem_id, Utc::now())
        .await
        .unwrap_err()
        .is_retryable());
    b.retract_memory_facts(&p, mem_id, Utc::now())
        .await
        .unwrap();
    let active = b
        .neighborhood(&p, &[alice.id], 1, 10, &TemporalFilter::default())
        .await
        .unwrap();
    assert!(active.edges.is_empty());
}

#[test]
fn jittered_backoff_stays_within_bounds() {
    let max = Duration::from_millis(100);
    assert_eq!(jittered(Duration::ZERO, 42), Duration::ZERO);
    for seed in [0u64, 1, 7, 12345, u64::MAX] {
        let d = jittered(max, seed);
        assert!(d <= max, "jittered {d:?} exceeded {max:?}");
    }
}
