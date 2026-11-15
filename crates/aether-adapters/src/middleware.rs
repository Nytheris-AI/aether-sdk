//! Shared outbound connector middleware decorators: `Batcher`, `Timeout`, `Resilient`, `Traced`.

use std::future::Future;
use std::time::Duration;

use aether_core::{
    AetherError, ConnectorCapabilities, ConnectorHealth, DistanceMetric, Entity, EntityId,
    FilterExpr, GraphStore, KvStore, MemoryId, MemoryItem, MemoryReceipt, MemoryScope,
    OutboundConnector, ProjectId, Relation, Result, Subgraph, TemporalFilter, VectorMatch,
    VectorRecord, VectorStore,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tracing::Instrument;

/// Retry configuration for transient outbound failures.
#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    pub max_retries: usize,
    pub base_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_delay: Duration::from_millis(50),
        }
    }
}

async fn retry_transient<F, Fut, T>(policy: RetryPolicy, mut op: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T>>,
{
    let mut attempt = 0;
    loop {
        match op().await {
            Ok(val) => return Ok(val),
            Err(err) if err.is_retryable() && attempt < policy.max_retries => {
                let shift = attempt.min(10) as u32;
                let backoff = policy.base_delay.saturating_mul(1u32 << shift);
                tokio::time::sleep(backoff).await;
                attempt += 1;
            }
            Err(err) => return Err(err),
        }
    }
}

/// Retries transient errors with exponential backoff; fails fast on permanent errors.
pub struct Resilient<T> {
    inner: T,
    policy: RetryPolicy,
}

impl<T> Resilient<T> {
    pub fn new(inner: T, policy: RetryPolicy) -> Self {
        Self { inner, policy }
    }
}

#[async_trait]
impl<T: OutboundConnector> OutboundConnector for Resilient<T> {
    fn name(&self) -> &'static str {
        self.inner.name()
    }
    fn capabilities(&self) -> ConnectorCapabilities {
        self.inner.capabilities()
    }
    async fn health_check(&self) -> Result<ConnectorHealth> {
        retry_transient(self.policy, || self.inner.health_check()).await
    }
    async fn close(&self) -> Result<()> {
        self.inner.close().await
    }
}

#[async_trait]
impl<T: KvStore> KvStore for Resilient<T> {
    async fn get(&self, ns: &ProjectId, id: MemoryId) -> Result<Option<MemoryItem>> {
        retry_transient(self.policy, || self.inner.get(ns, id)).await
    }

    async fn put(&self, ns: &ProjectId, item: &MemoryItem, ttl: Option<Duration>) -> Result<()> {
        retry_transient(self.policy, || self.inner.put(ns, item, ttl)).await
    }

    async fn put_if_absent(
        &self,
        ns: &ProjectId,
        idempotency_key: &str,
        receipt: &MemoryReceipt,
        ttl: Option<Duration>,
    ) -> Result<Option<MemoryReceipt>> {
        retry_transient(self.policy, || {
            self.inner
                .put_if_absent(ns, idempotency_key, receipt, ttl)
        })
        .await
    }

    async fn cas_update(
        &self,
        ns: &ProjectId,
        id: MemoryId,
        expected_version: Option<u64>,
        new_item: &MemoryItem,
    ) -> Result<()> {
        retry_transient(self.policy, || {
            self.inner.cas_update(ns, id, expected_version, new_item)
        })
        .await
    }

    async fn recent(
        &self,
        ns: &ProjectId,
        scope: Option<MemoryScope>,
        limit: usize,
    ) -> Result<Vec<MemoryItem>> {
        retry_transient(self.policy, || self.inner.recent(ns, scope, limit)).await
    }

    async fn delete(&self, ns: &ProjectId, id: MemoryId) -> Result<()> {
        retry_transient(self.policy, || self.inner.delete(ns, id)).await
    }
}

#[async_trait]
impl<T: VectorStore> VectorStore for Resilient<T> {
    async fn initialize(
        &self,
        ns: &ProjectId,
        dimensions: usize,
        metric: DistanceMetric,
    ) -> Result<()> {
        retry_transient(self.policy, || self.inner.initialize(ns, dimensions, metric)).await
    }

    async fn upsert_batch(&self, ns: &ProjectId, records: &[VectorRecord]) -> Result<()> {
        retry_transient(self.policy, || self.inner.upsert_batch(ns, records)).await
    }

    async fn delete(&self, ns: &ProjectId, ids: &[MemoryId]) -> Result<()> {
        retry_transient(self.policy, || self.inner.delete(ns, ids)).await
    }

    async fn search(
        &self,
        ns: &ProjectId,
        query_vector: &[f32],
        top_k: usize,
        filter: Option<&FilterExpr>,
    ) -> Result<Vec<VectorMatch>> {
        retry_transient(self.policy, || {
            self.inner.search(ns, query_vector, top_k, filter)
        })
        .await
    }
}

#[async_trait]
impl<T: GraphStore> GraphStore for Resilient<T> {
    async fn upsert_entities(&self, ns: &ProjectId, entities: &[Entity]) -> Result<()> {
        retry_transient(self.policy, || self.inner.upsert_entities(ns, entities)).await
    }

    async fn upsert_relations(
        &self,
        ns: &ProjectId,
        source_memory_id: MemoryId,
        relations: &[Relation],
    ) -> Result<()> {
        retry_transient(self.policy, || {
            self.inner.upsert_relations(ns, source_memory_id, relations)
        })
        .await
    }

    async fn retract_memory_facts(
        &self,
        ns: &ProjectId,
        source_memory_id: MemoryId,
        invalidated_at: DateTime<Utc>,
    ) -> Result<()> {
        retry_transient(self.policy, || {
            self.inner
                .retract_memory_facts(ns, source_memory_id, invalidated_at)
        })
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
        retry_transient(self.policy, || {
            self.inner
                .neighborhood(ns, seeds, hops, per_node_limit, temporal)
        })
        .await
    }
}

/// Bounds per-operation tail latency, mapping elapsed timeouts to `AetherError::Transient`.
pub struct Timeout<T> {
    inner: T,
    duration: Duration,
}

impl<T> Timeout<T> {
    pub fn new(inner: T, duration: Duration) -> Self {
        Self { inner, duration }
    }

    async fn run<R, Fut: Future<Output = Result<R>>>(
        &self,
        connector: &'static str,
        fut: Fut,
    ) -> Result<R> {
        match tokio::time::timeout(self.duration, fut).await {
            Ok(res) => res,
            Err(_) => Err(AetherError::Transient {
                connector,
                message: format!("operation timed out after {:?}", self.duration),
            }),
        }
    }
}

#[async_trait]
impl<T: OutboundConnector> OutboundConnector for Timeout<T> {
    fn name(&self) -> &'static str {
        self.inner.name()
    }
    fn capabilities(&self) -> ConnectorCapabilities {
        self.inner.capabilities()
    }
    async fn health_check(&self) -> Result<ConnectorHealth> {
        self.run(self.inner.name(), self.inner.health_check()).await
    }
    async fn close(&self) -> Result<()> {
        self.inner.close().await
    }
}

#[async_trait]
impl<T: KvStore> KvStore for Timeout<T> {
    async fn get(&self, ns: &ProjectId, id: MemoryId) -> Result<Option<MemoryItem>> {
        self.run(self.inner.name(), self.inner.get(ns, id)).await
    }

    async fn put(&self, ns: &ProjectId, item: &MemoryItem, ttl: Option<Duration>) -> Result<()> {
        self.run(self.inner.name(), self.inner.put(ns, item, ttl))
            .await
    }

    async fn put_if_absent(
        &self,
        ns: &ProjectId,
        idempotency_key: &str,
        receipt: &MemoryReceipt,
        ttl: Option<Duration>,
    ) -> Result<Option<MemoryReceipt>> {
        self.run(
            self.inner.name(),
            self.inner
                .put_if_absent(ns, idempotency_key, receipt, ttl),
        )
        .await
    }

    async fn cas_update(
        &self,
        ns: &ProjectId,
        id: MemoryId,
        expected_version: Option<u64>,
        new_item: &MemoryItem,
    ) -> Result<()> {
        self.run(
            self.inner.name(),
            self.inner.cas_update(ns, id, expected_version, new_item),
        )
        .await
    }

    async fn recent(
        &self,
        ns: &ProjectId,
        scope: Option<MemoryScope>,
        limit: usize,
    ) -> Result<Vec<MemoryItem>> {
        self.run(self.inner.name(), self.inner.recent(ns, scope, limit))
            .await
    }

    async fn delete(&self, ns: &ProjectId, id: MemoryId) -> Result<()> {
        self.run(self.inner.name(), self.inner.delete(ns, id))
            .await
    }
}

#[async_trait]
impl<T: VectorStore> VectorStore for Timeout<T> {
    async fn initialize(
        &self,
        ns: &ProjectId,
        dimensions: usize,
        metric: DistanceMetric,
    ) -> Result<()> {
        self.run(self.inner.name(), self.inner.initialize(ns, dimensions, metric))
            .await
    }

    async fn upsert_batch(&self, ns: &ProjectId, records: &[VectorRecord]) -> Result<()> {
        self.run(self.inner.name(), self.inner.upsert_batch(ns, records))
            .await
    }

    async fn delete(&self, ns: &ProjectId, ids: &[MemoryId]) -> Result<()> {
        self.run(self.inner.name(), self.inner.delete(ns, ids)).await
    }

    async fn search(
        &self,
        ns: &ProjectId,
        query_vector: &[f32],
        top_k: usize,
        filter: Option<&FilterExpr>,
    ) -> Result<Vec<VectorMatch>> {
        self.run(
            self.inner.name(),
            self.inner.search(ns, query_vector, top_k, filter),
        )
        .await
    }
}

#[async_trait]
impl<T: GraphStore> GraphStore for Timeout<T> {
    async fn upsert_entities(&self, ns: &ProjectId, entities: &[Entity]) -> Result<()> {
        self.run(self.inner.name(), self.inner.upsert_entities(ns, entities))
            .await
    }

    async fn upsert_relations(
        &self,
        ns: &ProjectId,
        source_memory_id: MemoryId,
        relations: &[Relation],
    ) -> Result<()> {
        self.run(
            self.inner.name(),
            self.inner.upsert_relations(ns, source_memory_id, relations),
        )
        .await
    }

    async fn retract_memory_facts(
        &self,
        ns: &ProjectId,
        source_memory_id: MemoryId,
        invalidated_at: DateTime<Utc>,
    ) -> Result<()> {
        self.run(
            self.inner.name(),
            self.inner
                .retract_memory_facts(ns, source_memory_id, invalidated_at),
        )
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
        self.run(
            self.inner.name(),
            self.inner
                .neighborhood(ns, seeds, hops, per_node_limit, temporal),
        )
        .await
    }
}

/// Automatically chunks `upsert_batch` calls according to `capabilities().max_batch_size`.
pub struct Batcher<T> {
    inner: T,
}

impl<T> Batcher<T> {
    pub fn new(inner: T) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl<T: OutboundConnector> OutboundConnector for Batcher<T> {
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
impl<T: VectorStore> VectorStore for Batcher<T> {
    async fn initialize(
        &self,
        ns: &ProjectId,
        dimensions: usize,
        metric: DistanceMetric,
    ) -> Result<()> {
        self.inner.initialize(ns, dimensions, metric).await
    }

    async fn upsert_batch(&self, ns: &ProjectId, records: &[VectorRecord]) -> Result<()> {
        let chunk_size = self.inner.capabilities().max_batch_size.max(1);
        for chunk in records.chunks(chunk_size) {
            self.inner.upsert_batch(ns, chunk).await?;
        }
        Ok(())
    }

    async fn delete(&self, ns: &ProjectId, ids: &[MemoryId]) -> Result<()> {
        self.inner.delete(ns, ids).await
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

/// Emits structured `tracing` spans tagged with connector name, project_id, and operation.
pub struct Traced<T> {
    inner: T,
}

impl<T> Traced<T> {
    pub fn new(inner: T) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl<T: OutboundConnector> OutboundConnector for Traced<T> {
    fn name(&self) -> &'static str {
        self.inner.name()
    }
    fn capabilities(&self) -> ConnectorCapabilities {
        self.inner.capabilities()
    }
    async fn health_check(&self) -> Result<ConnectorHealth> {
        let span = tracing::info_span!("connector_op", connector = self.inner.name(), op = "health_check");
        self.inner.health_check().instrument(span).await
    }
    async fn close(&self) -> Result<()> {
        let span = tracing::info_span!("connector_op", connector = self.inner.name(), op = "close");
        self.inner.close().instrument(span).await
    }
}

#[async_trait]
impl<T: KvStore> KvStore for Traced<T> {
    async fn get(&self, ns: &ProjectId, id: MemoryId) -> Result<Option<MemoryItem>> {
        let span = tracing::info_span!("connector_op", connector = self.inner.name(), project_id = %ns, op = "kv_get");
        self.inner.get(ns, id).instrument(span).await
    }

    async fn put(&self, ns: &ProjectId, item: &MemoryItem, ttl: Option<Duration>) -> Result<()> {
        let span = tracing::info_span!("connector_op", connector = self.inner.name(), project_id = %ns, op = "kv_put");
        self.inner.put(ns, item, ttl).instrument(span).await
    }

    async fn put_if_absent(
        &self,
        ns: &ProjectId,
        idempotency_key: &str,
        receipt: &MemoryReceipt,
        ttl: Option<Duration>,
    ) -> Result<Option<MemoryReceipt>> {
        let span = tracing::info_span!("connector_op", connector = self.inner.name(), project_id = %ns, op = "kv_put_if_absent");
        self.inner
            .put_if_absent(ns, idempotency_key, receipt, ttl)
            .instrument(span)
            .await
    }

    async fn cas_update(
        &self,
        ns: &ProjectId,
        id: MemoryId,
        expected_version: Option<u64>,
        new_item: &MemoryItem,
    ) -> Result<()> {
        let span = tracing::info_span!("connector_op", connector = self.inner.name(), project_id = %ns, op = "kv_cas_update");
        self.inner
            .cas_update(ns, id, expected_version, new_item)
            .instrument(span)
            .await
    }

    async fn recent(
        &self,
        ns: &ProjectId,
        scope: Option<MemoryScope>,
        limit: usize,
    ) -> Result<Vec<MemoryItem>> {
        let span = tracing::info_span!("connector_op", connector = self.inner.name(), project_id = %ns, op = "kv_recent", limit);
        self.inner.recent(ns, scope, limit).instrument(span).await
    }

    async fn delete(&self, ns: &ProjectId, id: MemoryId) -> Result<()> {
        let span = tracing::info_span!("connector_op", connector = self.inner.name(), project_id = %ns, op = "kv_delete");
        self.inner.delete(ns, id).instrument(span).await
    }
}

#[async_trait]
impl<T: VectorStore> VectorStore for Traced<T> {
    async fn initialize(
        &self,
        ns: &ProjectId,
        dimensions: usize,
        metric: DistanceMetric,
    ) -> Result<()> {
        let span = tracing::info_span!("connector_op", connector = self.inner.name(), project_id = %ns, op = "initialize");
        self.inner.initialize(ns, dimensions, metric).instrument(span).await
    }

    async fn upsert_batch(&self, ns: &ProjectId, records: &[VectorRecord]) -> Result<()> {
        let span = tracing::info_span!("connector_op", connector = self.inner.name(), project_id = %ns, op = "upsert_batch", count = records.len());
        self.inner.upsert_batch(ns, records).instrument(span).await
    }

    async fn delete(&self, ns: &ProjectId, ids: &[MemoryId]) -> Result<()> {
        let span = tracing::info_span!("connector_op", connector = self.inner.name(), project_id = %ns, op = "delete", count = ids.len());
        self.inner.delete(ns, ids).instrument(span).await
    }

    async fn search(
        &self,
        ns: &ProjectId,
        query_vector: &[f32],
        top_k: usize,
        filter: Option<&FilterExpr>,
    ) -> Result<Vec<VectorMatch>> {
        let span = tracing::info_span!("connector_op", connector = self.inner.name(), project_id = %ns, op = "search", top_k);
        self.inner.search(ns, query_vector, top_k, filter).instrument(span).await
    }
}

#[async_trait]
impl<T: GraphStore> GraphStore for Traced<T> {
    async fn upsert_entities(&self, ns: &ProjectId, entities: &[Entity]) -> Result<()> {
        let span = tracing::info_span!("connector_op", connector = self.inner.name(), project_id = %ns, op = "upsert_entities", count = entities.len());
        self.inner.upsert_entities(ns, entities).instrument(span).await
    }

    async fn upsert_relations(
        &self,
        ns: &ProjectId,
        source_memory_id: MemoryId,
        relations: &[Relation],
    ) -> Result<()> {
        let span = tracing::info_span!("connector_op", connector = self.inner.name(), project_id = %ns, op = "upsert_relations", count = relations.len());
        self.inner
            .upsert_relations(ns, source_memory_id, relations)
            .instrument(span)
            .await
    }

    async fn retract_memory_facts(
        &self,
        ns: &ProjectId,
        source_memory_id: MemoryId,
        invalidated_at: DateTime<Utc>,
    ) -> Result<()> {
        let span = tracing::info_span!("connector_op", connector = self.inner.name(), project_id = %ns, op = "retract_memory_facts");
        self.inner
            .retract_memory_facts(ns, source_memory_id, invalidated_at)
            .instrument(span)
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
        let span = tracing::info_span!("connector_op", connector = self.inner.name(), project_id = %ns, op = "neighborhood", hops);
        self.inner
            .neighborhood(ns, seeds, hops, per_node_limit, temporal)
            .instrument(span)
            .await
    }
}
