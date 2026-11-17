mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use aether_adapters::{FaultPoint, InMemoryBackend, Resilient, RetryPolicy, Timeout, Traced};
use aether_core::{
    AetherError, ConnectorCapabilities, ConnectorHealth, DistanceMetric, Entity, EntityId,
    FilterExpr, GraphStore, MemoryId, MemoryInbound, MemoryInput, MemoryItem, MemoryReceipt,
    MemoryScope, MemoryUpdate, OutboundConnector, ProjectId, RecallMode, RecallQuery, ReceiptState,
    RecordStore, Relation, Result, Subgraph, TemporalFilter, VectorMatch, VectorRecord,
    VectorStore, WriteDurability,
};
use aether_sdk::MemoryManager;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use common::{build_fault_injectable_manager, embed_text, EMBED_DIM};
use serde_json::json;

#[derive(Clone, Copy)]
enum FaultMode {
    Transient { fail_count: usize },
    Permanent,
}

struct CountingFaultStore {
    inner: Arc<InMemoryBackend>,
    record_put_calls: AtomicUsize,
    vector_upsert_calls: AtomicUsize,
    graph_upsert_calls: AtomicUsize,
    record_put_mode: Option<FaultMode>,
    vector_upsert_mode: Option<FaultMode>,
    graph_upsert_mode: Option<FaultMode>,
}

impl CountingFaultStore {
    fn new(inner: Arc<InMemoryBackend>) -> Self {
        Self {
            inner,
            record_put_calls: AtomicUsize::new(0),
            vector_upsert_calls: AtomicUsize::new(0),
            graph_upsert_calls: AtomicUsize::new(0),
            record_put_mode: None,
            vector_upsert_mode: None,
            graph_upsert_mode: None,
        }
    }

    fn maybe_fail(counter: &AtomicUsize, mode: Option<FaultMode>, op: &str) -> Result<()> {
        let attempt = counter.fetch_add(1, Ordering::SeqCst) + 1;
        match mode {
            Some(FaultMode::Transient { fail_count }) if attempt <= fail_count => {
                Err(AetherError::Transient {
                    connector: "fault_probe",
                    message: format!("{op} transient fault on attempt {attempt}"),
                })
            }
            Some(FaultMode::Permanent) => Err(AetherError::Permanent {
                connector: "fault_probe",
                message: format!("{op} permanent fault"),
            }),
            _ => Ok(()),
        }
    }
}

#[async_trait]
impl OutboundConnector for CountingFaultStore {
    fn name(&self) -> &'static str {
        self.inner.name()
    }
    fn capabilities(&self) -> ConnectorCapabilities {
        self.inner.capabilities()
    }
    async fn health_check(&self) -> Result<ConnectorHealth> {
        self.inner.health_check().await
    }
    async fn close(&self) -> Result<()> {
        self.inner.close().await
    }
}

#[async_trait]
impl RecordStore for CountingFaultStore {
    async fn get(&self, ns: &ProjectId, id: MemoryId) -> Result<Option<MemoryItem>> {
        self.inner.get(ns, id).await
    }
    async fn put(&self, ns: &ProjectId, item: &MemoryItem, ttl: Option<Duration>) -> Result<()> {
        Self::maybe_fail(&self.record_put_calls, self.record_put_mode, "record_put")?;
        self.inner.put(ns, item, ttl).await
    }
    async fn put_if_absent(
        &self,
        ns: &ProjectId,
        idempotency_key: &str,
        receipt: &MemoryReceipt,
        ttl: Option<Duration>,
    ) -> Result<Option<MemoryReceipt>> {
        self.inner
            .put_if_absent(ns, idempotency_key, receipt, ttl)
            .await
    }
    async fn cas_update(
        &self,
        ns: &ProjectId,
        id: MemoryId,
        expected_version: Option<u64>,
        new_item: &MemoryItem,
    ) -> Result<()> {
        self.inner
            .cas_update(ns, id, expected_version, new_item)
            .await
    }
    async fn recent(
        &self,
        ns: &ProjectId,
        scope: Option<MemoryScope>,
        limit: usize,
    ) -> Result<Vec<MemoryItem>> {
        self.inner.recent(ns, scope, limit).await
    }
    async fn delete(&self, ns: &ProjectId, id: MemoryId) -> Result<()> {
        RecordStore::delete(&*self.inner, ns, id).await
    }
}

#[async_trait]
impl VectorStore for CountingFaultStore {
    async fn initialize(
        &self,
        ns: &ProjectId,
        dimensions: usize,
        metric: DistanceMetric,
    ) -> Result<()> {
        self.inner.initialize(ns, dimensions, metric).await
    }
    async fn upsert_batch(&self, ns: &ProjectId, records: &[VectorRecord]) -> Result<()> {
        Self::maybe_fail(
            &self.vector_upsert_calls,
            self.vector_upsert_mode,
            "vector_upsert",
        )?;
        self.inner.upsert_batch(ns, records).await
    }
    async fn delete(&self, ns: &ProjectId, ids: &[MemoryId]) -> Result<()> {
        VectorStore::delete(&*self.inner, ns, ids).await
    }
    async fn search(
        &self,
        ns: &ProjectId,
        query_vector: &[f32],
        top_k: usize,
        filter: Option<&FilterExpr>,
    ) -> Result<Vec<VectorMatch>> {
        self.inner.search(ns, query_vector, top_k, filter).await
    }
}

#[async_trait]
impl GraphStore for CountingFaultStore {
    async fn upsert_entities(&self, ns: &ProjectId, entities: &[Entity]) -> Result<()> {
        Self::maybe_fail(
            &self.graph_upsert_calls,
            self.graph_upsert_mode,
            "graph_upsert",
        )?;
        self.inner.upsert_entities(ns, entities).await
    }
    async fn upsert_relations(
        &self,
        ns: &ProjectId,
        source_memory_id: MemoryId,
        relations: &[Relation],
    ) -> Result<()> {
        self.inner
            .upsert_relations(ns, source_memory_id, relations)
            .await
    }
    async fn retract_memory_facts(
        &self,
        ns: &ProjectId,
        source_memory_id: MemoryId,
        invalidated_at: DateTime<Utc>,
    ) -> Result<()> {
        self.inner
            .retract_memory_facts(ns, source_memory_id, invalidated_at)
            .await
    }
    async fn neighborhood(
        &self,
        ns: &ProjectId,
        seeds: &[EntityId],
        hops: u8,
        per_node_limit: usize,
        temporal: &TemporalFilter,
    ) -> Result<Subgraph> {
        self.inner
            .neighborhood(ns, seeds, hops, per_node_limit, temporal)
            .await
    }
}

fn wrap_counting_manager(store: Arc<CountingFaultStore>) -> Arc<MemoryManager> {
    let policy = RetryPolicy {
        max_retries: 3,
        base_delay: Duration::from_millis(10),
    };
    let record_store = Arc::new(Traced::new(Resilient::new(
        Timeout::new(store.clone(), Duration::from_secs(2)),
        policy,
    )));
    let vec_store = Arc::new(Traced::new(Resilient::new(
        Timeout::new(store.clone(), Duration::from_secs(2)),
        policy,
    )));
    let graph_store = Arc::new(Traced::new(Resilient::new(
        Timeout::new(store, Duration::from_secs(2)),
        policy,
    )));
    Arc::new(MemoryManager::new(record_store, vec_store, graph_store))
}

fn sample_graph_input(pid: &ProjectId, content: &str) -> MemoryInput {
    let alice = Entity {
        id: EntityId::new("person:alice").unwrap(),
        name: "Alice".into(),
        entity_type: "Person".into(),
    };
    let service = Entity {
        id: EntityId::new("service:gateway").unwrap(),
        name: "Gateway".into(),
        entity_type: "Service".into(),
    };
    let mut input = MemoryInput::new(pid.clone(), content);
    input.embedding = Some(embed_text(content));
    input.durability = WriteDurability::WaitFlush;
    input.entities = vec![alice.clone(), service.clone()];
    input.relations = vec![Relation {
        source: alice.id,
        target: service.id,
        relation_type: "OWNS".into(),
        valid_at: Utc::now(),
        invalid_at: None,
    }];
    input
}

#[tokio::test]
async fn e2e_transient_faults_across_all_stores_are_retried_and_succeed() {
    let raw = Arc::new(InMemoryBackend::new());
    let pid = ProjectId::new("fault-transient").unwrap();
    raw.initialize(&pid, EMBED_DIM, DistanceMetric::Cosine)
        .await
        .unwrap();

    // 1. RecordStore transient failure recovers on 3rd attempt.
    let mut counting = CountingFaultStore::new(raw.clone());
    counting.record_put_mode = Some(FaultMode::Transient { fail_count: 2 });
    let counting = Arc::new(counting);
    let counting_mgr = wrap_counting_manager(counting.clone());

    let r1 = counting_mgr
        .remember(sample_graph_input(&pid, "Alice owns the gateway service"))
        .await
        .unwrap();
    assert_eq!(r1.state, ReceiptState::Persisted);
    assert_eq!(counting.record_put_calls.load(Ordering::SeqCst), 3);
    counting_mgr.shutdown().await.unwrap();

    // 2. VectorStore + GraphStore FaultPoint transient failures recover via Resilient<Timeout<...>>.
    let manager = build_fault_injectable_manager(raw.clone());
    raw.fail_next(FaultPoint::VectorUpsert, 2);
    raw.fail_next(FaultPoint::GraphUpsert, 2);

    let r2 = manager
        .remember(sample_graph_input(&pid, "Gateway handles retryable faults"))
        .await
        .unwrap();
    assert_eq!(r2.state, ReceiptState::Persisted);
    assert!(raw.has_vector(&pid, r2.memory_id));

    let mut q = RecallQuery::new(pid.clone(), "Gateway retryable faults");
    q.query_vector = Some(embed_text("Gateway retryable faults"));
    q.seed_entities = vec![EntityId::new("person:alice").unwrap()];
    q.mode = RecallMode::Hybrid;
    q.top_k = 5;
    let res = manager.recall(q).await.unwrap();
    assert!(res.hits.iter().any(|h| h.memory_id == r2.memory_id));
    assert!(!res.subgraph.edges.is_empty());

    // 3. Exceeding max_retries (3) surfaces AetherError::Transient.
    raw.fail_next(FaultPoint::VectorUpsert, 4);
    let err = manager
        .remember(sample_graph_input(&pid, "Exceeds retry budget"))
        .await
        .unwrap_err();
    assert!(matches!(err, AetherError::Transient { .. }));

    manager.shutdown().await.unwrap();
}

#[tokio::test]
async fn e2e_permanent_failures_fail_fast_without_retrying() {
    let raw = Arc::new(InMemoryBackend::new());
    let pid = ProjectId::new("fault-permanent").unwrap();
    raw.initialize(&pid, EMBED_DIM, DistanceMetric::Cosine)
        .await
        .unwrap();

    // RecordStore permanent failure: 1 attempt, no retries.
    let mut rec_store = CountingFaultStore::new(raw.clone());
    rec_store.record_put_mode = Some(FaultMode::Permanent);
    let rec_store = Arc::new(rec_store);
    let rec_mgr = wrap_counting_manager(rec_store.clone());
    let err = rec_mgr
        .remember(sample_graph_input(&pid, "permanent record failure"))
        .await
        .unwrap_err();
    assert!(matches!(err, AetherError::Permanent { .. }));
    assert_eq!(rec_store.record_put_calls.load(Ordering::SeqCst), 1);
    rec_mgr.shutdown().await.unwrap();

    // VectorStore permanent failure: 1 attempt, no retries.
    let mut vec_store = CountingFaultStore::new(raw.clone());
    vec_store.vector_upsert_mode = Some(FaultMode::Permanent);
    let vec_store = Arc::new(vec_store);
    let vec_mgr = wrap_counting_manager(vec_store.clone());
    let err = vec_mgr
        .remember(sample_graph_input(&pid, "permanent vector failure"))
        .await
        .unwrap_err();
    assert!(matches!(err, AetherError::Permanent { .. }));
    assert_eq!(vec_store.vector_upsert_calls.load(Ordering::SeqCst), 1);
    // Clear pending item before shutdown so shutdown flush doesn't re-hit the permanent fault.
    let _ = vec_mgr.forget(&pid, MemoryId::new()).await;

    // GraphStore permanent failure: 1 attempt, no retries.
    let mut graph_store = CountingFaultStore::new(raw);
    graph_store.graph_upsert_mode = Some(FaultMode::Permanent);
    let graph_store = Arc::new(graph_store);
    let graph_mgr = wrap_counting_manager(graph_store.clone());
    let err = graph_mgr
        .remember(sample_graph_input(&pid, "permanent graph failure"))
        .await
        .unwrap_err();
    assert!(matches!(err, AetherError::Permanent { .. }));
    assert_eq!(graph_store.graph_upsert_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn e2e_update_and_forget_stay_consistent_after_transient_recovery() {
    let raw = Arc::new(InMemoryBackend::new());
    let manager = build_fault_injectable_manager(raw.clone());
    let pid = ProjectId::new("fault-lifecycle").unwrap();
    raw.initialize(&pid, EMBED_DIM, DistanceMetric::Cosine)
        .await
        .unwrap();

    raw.fail_next(FaultPoint::VectorUpsert, 2);
    raw.fail_next(FaultPoint::GraphUpsert, 2);
    let receipt = manager
        .remember(sample_graph_input(&pid, "Initial gateway configuration"))
        .await
        .unwrap();
    assert_eq!(receipt.version, 1);

    // Update while VectorUpsert experiences transient faults.
    raw.fail_next(FaultPoint::VectorUpsert, 2);
    let updated = manager
        .update(
            &pid,
            receipt.memory_id,
            MemoryUpdate {
                content: Some("Updated gateway configuration v2".into()),
                embedding: Some(embed_text("Updated gateway configuration v2")),
                metadata: Some(json!({ "rev": 2 })),
                expected_version: Some(1),
            },
        )
        .await
        .unwrap();
    assert_eq!(updated.version, 2);

    let mut q = RecallQuery::new(pid.clone(), "Updated gateway configuration v2");
    q.query_vector = Some(embed_text("Updated gateway configuration v2"));
    q.mode = RecallMode::Vector;
    q.top_k = 5;
    let res = manager.recall(q.clone()).await.unwrap();
    let hit = res
        .hits
        .iter()
        .find(|h| h.memory_id == receipt.memory_id)
        .expect("updated memory should be recalled");
    assert_eq!(hit.content, "Updated gateway configuration v2");
    assert_eq!(hit.metadata["rev"], 2);

    // Stale CAS update must still conflict.
    let conflict = manager
        .update(
            &pid,
            receipt.memory_id,
            MemoryUpdate {
                content: Some("stale".into()),
                embedding: None,
                metadata: None,
                expected_version: Some(1),
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(conflict, AetherError::Conflict { .. }));

    // Forget while both VectorDelete and GraphRetract experience transient faults.
    raw.fail_next(FaultPoint::VectorDelete, 2);
    raw.fail_next(FaultPoint::GraphRetract, 2);
    manager.forget(&pid, receipt.memory_id).await.unwrap();

    assert!(!raw.has_vector(&pid, receipt.memory_id));
    let after_forget = manager.recall(q).await.unwrap();
    assert!(after_forget
        .hits
        .iter()
        .all(|h| h.memory_id != receipt.memory_id));

    let mut graph_q = RecallQuery::new(pid.clone(), "Alice");
    graph_q.mode = RecallMode::Graph;
    graph_q.seed_entities = vec![EntityId::new("person:alice").unwrap()];
    graph_q.temporal = TemporalFilter {
        as_of: None,
        active_only: true,
    };
    let graph_res = manager.recall(graph_q).await.unwrap();
    assert!(graph_res.subgraph.edges.is_empty());

    let not_found = manager
        .update(
            &pid,
            receipt.memory_id,
            MemoryUpdate {
                content: Some("after forget".into()),
                embedding: None,
                metadata: None,
                expected_version: None,
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(not_found, AetherError::NotFound(_)));

    manager.shutdown().await.unwrap();
}
