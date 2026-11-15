//! Public facade for Aether SDK: unified AI memory storage & database gateway.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use aether_core::{
    AetherError, GraphStore, MemoryId, MemoryInbound, MemoryInput, MemoryItem, MemoryReceipt,
    MemoryUpdate, ProjectId, RecallQuery, RecallResult, Result, VectorStore, WriteDurability,
};
use aether_cortex::{
    HotContextCache, IngestionBuffer, PolicyDecision, PolicyRouter, QueryPlanner,
};
use async_trait::async_trait;
use chrono::Utc;

pub use aether_core as core;
pub use aether_adapters as adapters;

/// Primary non-generic SDK facade implementing `MemoryInbound`.
pub struct MemoryManager {
    vector_store: Arc<dyn VectorStore>,
    graph_store: Arc<dyn GraphStore>,
    policy: PolicyRouter,
    hot_cache: Arc<HotContextCache>,
    buffer: Arc<IngestionBuffer>,
    planner: QueryPlanner,
    versions: RwLock<HashMap<(ProjectId, MemoryId), MemoryItem>>,
}

impl MemoryManager {
    pub fn new(vector_store: Arc<dyn VectorStore>, graph_store: Arc<dyn GraphStore>) -> Self {
        let hot_cache = Arc::new(HotContextCache::new(256));
        let buffer = Arc::new(IngestionBuffer::new(
            vector_store.clone(),
            graph_store.clone(),
        ));
        let planner = QueryPlanner::new(
            vector_store.clone(),
            graph_store.clone(),
            hot_cache.clone(),
            buffer.clone(),
        );
        Self {
            vector_store,
            graph_store,
            policy: PolicyRouter::new(),
            hot_cache,
            buffer,
            planner,
            versions: RwLock::new(HashMap::new()),
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

        {
            let mut map = self.versions.write().unwrap();
            map.insert((item.project_id.clone(), item.id), item.clone());
        }
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
        let updated = {
            let mut map = self.versions.write().unwrap();
            let current = map
                .get_mut(&(project_id.clone(), memory_id))
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
            current.clone()
        };

        self.hot_cache.insert(updated.clone());
        self.buffer.enqueue(updated, WriteDurability::WaitFlush).await
    }

    async fn forget(&self, project_id: &ProjectId, memory_id: MemoryId) -> Result<()> {
        {
            let mut map = self.versions.write().unwrap();
            map.remove(&(project_id.clone(), memory_id));
        }
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
        self.buffer.shutdown().await
    }
}
