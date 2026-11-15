//! Public facade for Aether SDK: unified AI memory storage & database gateway.

use std::sync::Arc;
use std::time::Duration;

use aether_core::{
    AetherError, GraphStore, KvStore, MemoryId, MemoryInbound, MemoryInput,
    MemoryReceipt, MemoryScope, MemoryUpdate, ProjectId, RecallQuery, RecallResult, ReceiptState,
    Result, VectorStore, WriteDurability,
};
use aether_cortex::{
    HotContextCache, IngestionBuffer, PolicyDecision, PolicyRouter, QueryPlanner,
};
use async_trait::async_trait;
use chrono::Utc;

pub use aether_adapters as adapters;
pub use aether_core as core;

/// Primary non-generic SDK facade implementing `MemoryInbound` across L1/L2/L3 stores.
pub struct MemoryManager {
    kv_store: Arc<dyn KvStore>,
    vector_store: Arc<dyn VectorStore>,
    graph_store: Arc<dyn GraphStore>,
    policy: PolicyRouter,
    hot_cache: Arc<HotContextCache>,
    buffer: Arc<IngestionBuffer>,
    planner: QueryPlanner,
    session_ttl: Option<Duration>,
}

impl MemoryManager {
    pub fn new(
        kv_store: Arc<dyn KvStore>,
        vector_store: Arc<dyn VectorStore>,
        graph_store: Arc<dyn GraphStore>,
    ) -> Self {
        let hot_cache = Arc::new(HotContextCache::new(256));
        let buffer = Arc::new(IngestionBuffer::new(
            vector_store.clone(),
            graph_store.clone(),
        ));
        let planner = QueryPlanner::new(
            kv_store.clone(),
            vector_store.clone(),
            graph_store.clone(),
            hot_cache.clone(),
            buffer.clone(),
        );
        Self {
            kv_store,
            vector_store,
            graph_store,
            policy: PolicyRouter::new(),
            hot_cache,
            buffer,
            planner,
            session_ttl: Some(Duration::from_secs(3600)),
        }
    }

    fn ttl_for(&self, scope: MemoryScope) -> Option<Duration> {
        match scope {
            MemoryScope::Session => self.session_ttl,
            MemoryScope::Project | MemoryScope::User | MemoryScope::Global => None,
        }
    }
}

#[async_trait]
impl MemoryInbound for MemoryManager {
    async fn remember(&self, input: MemoryInput) -> Result<MemoryReceipt> {
        let durability = input.durability;
        let item = match self.policy.evaluate(&input)? {
            PolicyDecision::Duplicate(receipt) => return Ok(receipt),
            PolicyDecision::Accept(item) => item,
        };

        // Distributed atomic idempotency check via L2 KvStore.
        if let Some(key) = &item.idempotency_key {
            let candidate = MemoryReceipt {
                memory_id: item.id,
                state: ReceiptState::Buffered,
                version: item.version,
            };
            if let Some(existing) = self
                .kv_store
                .put_if_absent(&item.project_id, key, &candidate, self.ttl_for(item.scope))
                .await?
            {
                return Ok(existing);
            }
        }

        self.kv_store
            .put(&item.project_id, &item, self.ttl_for(item.scope))
            .await?;
        self.hot_cache.insert(item.clone());
        self.buffer.enqueue(item, durability).await
    }

    async fn recall(&self, query: RecallQuery) -> Result<RecallResult> {
        self.planner.plan_and_execute(&query).await
    }

    async fn update(
        &self,
        project_id: &ProjectId,
        memory_id: MemoryId,
        patch: MemoryUpdate,
    ) -> Result<MemoryReceipt> {
        let mut current = self
            .kv_store
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

        self.kv_store
            .cas_update(project_id, memory_id, patch.expected_version, &current)
            .await?;
        self.hot_cache.insert(current.clone());
        self.buffer
            .enqueue(current, WriteDurability::WaitFlush)
            .await
    }

    async fn forget(&self, project_id: &ProjectId, memory_id: MemoryId) -> Result<()> {
        self.kv_store.delete(project_id, memory_id).await?;
        self.hot_cache.remove(project_id, memory_id);
        self.buffer.remove_pending(project_id, memory_id);

        self.vector_store.delete(project_id, &[memory_id]).await?;
        self.graph_store
            .retract_memory_facts(project_id, memory_id, Utc::now())
            .await?;
        Ok(())
    }

    async fn flush(&self) -> Result<()> {
        self.buffer.flush().await
    }

    async fn shutdown(&self) -> Result<()> {
        self.buffer.shutdown().await?;
        self.kv_store.close().await?;
        Ok(())
    }
}
