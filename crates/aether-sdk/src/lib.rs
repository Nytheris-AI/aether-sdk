//! Public facade for Aether SDK: unified AI memory storage & database gateway.

use std::sync::Arc;
use std::time::Duration;

use aether_core::{
    AetherError, GraphStore, MemoryId, MemoryInbound, MemoryInput, MemoryReceipt, MemoryScope,
    MemoryUpdate, ProjectId, RecallQuery, RecallResult, ReceiptState, RecordStore, Result,
    VectorStore, WriteDurability,
};
use aether_cortex::{HotContextCache, IngestionBuffer, PolicyDecision, PolicyRouter, QueryPlanner};
use async_trait::async_trait;
use chrono::Utc;
use tracing::Instrument;

pub use aether_adapters as adapters;
pub use aether_core as core;

#[cfg(feature = "otel")]
pub mod telemetry;

/// Builder for [`MemoryManager`] with sensible production defaults (connector tracing enabled).
pub struct MemoryManagerBuilder {
    record_store: Arc<dyn RecordStore>,
    vector_store: Arc<dyn VectorStore>,
    graph_store: Arc<dyn GraphStore>,
    connector_tracing: bool,
    session_ttl: Option<Duration>,
}

impl MemoryManagerBuilder {
    /// Enables or disables automatic `Traced` connector span wrapping (enabled by default).
    pub fn with_connector_tracing(mut self, enabled: bool) -> Self {
        self.connector_tracing = enabled;
        self
    }

    /// Sets the TTL applied to `MemoryScope::Session` entries (default: 1 hour).
    pub fn with_session_ttl(mut self, ttl: Option<Duration>) -> Self {
        self.session_ttl = ttl;
        self
    }

    pub fn build(self) -> MemoryManager {
        let (record_store, vector_store, graph_store): (
            Arc<dyn RecordStore>,
            Arc<dyn VectorStore>,
            Arc<dyn GraphStore>,
        ) = if self.connector_tracing {
            (
                Arc::new(aether_adapters::Traced::new(self.record_store)),
                Arc::new(aether_adapters::Traced::new(self.vector_store)),
                Arc::new(aether_adapters::Traced::new(self.graph_store)),
            )
        } else {
            (self.record_store, self.vector_store, self.graph_store)
        };

        let hot_cache = Arc::new(HotContextCache::new(256));
        let buffer = Arc::new(IngestionBuffer::new(
            vector_store.clone(),
            graph_store.clone(),
        ));
        let planner = QueryPlanner::new(
            record_store.clone(),
            vector_store.clone(),
            graph_store.clone(),
            hot_cache.clone(),
            buffer.clone(),
        );
        MemoryManager {
            record_store,
            vector_store,
            graph_store,
            policy: PolicyRouter::new(),
            hot_cache,
            buffer,
            planner,
            session_ttl: self.session_ttl,
        }
    }
}

/// Primary non-generic SDK facade implementing `MemoryInbound` across L1/L2/L3 stores.
pub struct MemoryManager {
    record_store: Arc<dyn RecordStore>,
    vector_store: Arc<dyn VectorStore>,
    graph_store: Arc<dyn GraphStore>,
    policy: PolicyRouter,
    hot_cache: Arc<HotContextCache>,
    buffer: Arc<IngestionBuffer>,
    planner: QueryPlanner,
    session_ttl: Option<Duration>,
}

impl MemoryManager {
    pub fn builder(
        record_store: Arc<dyn RecordStore>,
        vector_store: Arc<dyn VectorStore>,
        graph_store: Arc<dyn GraphStore>,
    ) -> MemoryManagerBuilder {
        MemoryManagerBuilder {
            record_store,
            vector_store,
            graph_store,
            connector_tracing: true,
            session_ttl: Some(Duration::from_secs(3600)),
        }
    }

    pub fn new(
        record_store: Arc<dyn RecordStore>,
        vector_store: Arc<dyn VectorStore>,
        graph_store: Arc<dyn GraphStore>,
    ) -> Self {
        Self::builder(record_store, vector_store, graph_store).build()
    }

    fn ttl_for(&self, scope: MemoryScope) -> Option<Duration> {
        match scope {
            MemoryScope::Session => self.session_ttl,
            MemoryScope::Project | MemoryScope::User | MemoryScope::Global => None,
        }
    }
}

/// Runs an async operation inside `span` and records OpenTelemetry `otel.status_code`.
async fn run_spanned<T, Fut: std::future::Future<Output = Result<T>>>(
    span: tracing::Span,
    fut: Fut,
) -> Result<T> {
    let res = fut.instrument(span.clone()).await;
    match &res {
        Ok(_) => {
            span.record("otel.status_code", "OK");
        }
        Err(err) => {
            span.record("otel.status_code", "ERROR");
            let _entered = span.enter();
            tracing::error!(error = %err, "aether operation failed");
        }
    }
    res
}

#[async_trait]
impl MemoryInbound for MemoryManager {
    async fn remember(&self, input: MemoryInput) -> Result<MemoryReceipt> {
        let span = tracing::info_span!(
            "aether.remember",
            project_id = %input.project_id,
            scope = ?input.scope,
            durability = ?input.durability,
            otel.status_code = tracing::field::Empty,
        );
        run_spanned(span, async {
            let durability = input.durability;
            let item = match self.policy.evaluate(&input)? {
                PolicyDecision::Duplicate(receipt) => return Ok(receipt),
                PolicyDecision::Accept(item) => item,
            };

            // Distributed atomic idempotency check via L2 RecordStore.
            if let Some(key) = &item.idempotency_key {
                let candidate = MemoryReceipt {
                    memory_id: item.id,
                    state: ReceiptState::Buffered,
                    version: item.version,
                };
                if let Some(existing) = self
                    .record_store
                    .put_if_absent(&item.project_id, key, &candidate, self.ttl_for(item.scope))
                    .await?
                {
                    return Ok(existing);
                }
            }

            self.record_store
                .put(&item.project_id, &item, self.ttl_for(item.scope))
                .await?;
            self.hot_cache.insert(item.clone());
            self.buffer.enqueue(item, durability).await
        })
        .await
    }

    async fn recall(&self, query: RecallQuery) -> Result<RecallResult> {
        let span = tracing::info_span!(
            "aether.recall",
            project_id = %query.project_id,
            mode = ?query.mode,
            consistency = ?query.consistency,
            top_k = query.top_k,
            otel.status_code = tracing::field::Empty,
        );
        run_spanned(span, self.planner.plan_and_execute(&query)).await
    }

    async fn update(
        &self,
        project_id: &ProjectId,
        memory_id: MemoryId,
        patch: MemoryUpdate,
    ) -> Result<MemoryReceipt> {
        let span = tracing::info_span!(
            "aether.update",
            project_id = %project_id,
            memory_id = %memory_id,
            otel.status_code = tracing::field::Empty,
        );
        run_spanned(span, async {
            let mut current = self
                .record_store
                .get(project_id, memory_id)
                .await?
                .ok_or_else(|| AetherError::NotFound(format!("memory {memory_id}")))?;

            if let Some(expected) = patch.expected_version {
                if current.version != expected {
                    return Err(AetherError::Conflict {
                        memory_id,
                        expected_version: expected,
                        actual_version: current.version,
                    });
                }
            }

            if let Some(content) = patch.content {
                if content.trim().is_empty() {
                    return Err(AetherError::InvalidInput(
                        "updated content must not be empty".into(),
                    ));
                }
                current.content = content;
            }
            if let Some(embedding) = patch.embedding {
                current.embedding = Some(embedding);
            }
            if let Some(metadata) = patch.metadata {
                current.metadata = metadata;
            }
            current.version += 1;
            current.updated_at = Utc::now();

            self.record_store
                .cas_update(project_id, memory_id, patch.expected_version, &current)
                .await?;
            self.hot_cache.insert(current.clone());
            self.buffer
                .enqueue(current, WriteDurability::WaitFlush)
                .await
        })
        .await
    }

    async fn forget(&self, project_id: &ProjectId, memory_id: MemoryId) -> Result<()> {
        let span = tracing::info_span!(
            "aether.forget",
            project_id = %project_id,
            memory_id = %memory_id,
            otel.status_code = tracing::field::Empty,
        );
        run_spanned(span, async {
            self.record_store.delete(project_id, memory_id).await?;
            self.hot_cache.remove(project_id, memory_id);
            self.buffer.remove_pending(project_id, memory_id);

            self.vector_store.delete(project_id, &[memory_id]).await?;
            self.graph_store
                .retract_memory_facts(project_id, memory_id, Utc::now())
                .await?;
            Ok(())
        })
        .await
    }

    async fn flush(&self) -> Result<()> {
        let span = tracing::info_span!("aether.flush", otel.status_code = tracing::field::Empty,);
        run_spanned(span, self.buffer.flush()).await
    }

    async fn shutdown(&self) -> Result<()> {
        let span =
            tracing::info_span!("aether.shutdown", otel.status_code = tracing::field::Empty,);
        run_spanned(span, async {
            self.buffer.shutdown().await?;
            self.record_store.close().await?;
            Ok(())
        })
        .await
    }
}
