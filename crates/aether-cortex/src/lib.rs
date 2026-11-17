//! Gateway core orchestration for Aether SDK:
//! `PolicyRouter`, `HotContextCache`, `IngestionBuffer`, `QueryPlanner`, `Indexer`, and `PersonalizedPageRank`.

pub mod indexer;
pub mod ppr;

pub use indexer::{index_metadata, Indexer, PassReport};
pub use ppr::PersonalizedPageRank;

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, RwLock};

use aether_core::{
    AetherError, ConsistencyMode, DistanceMetric, EntityId, GraphStore, MemoryId, MemoryInput,
    MemoryItem, MemoryReceipt, ProjectId, RecallHit, RecallMode, RecallQuery, RecallResult,
    ReceiptState, RecordStore, Result, Subgraph, VectorRecord, VectorStore, WriteDurability,
};
use serde_json::Value;
use tracing::Instrument;

/// Result of deterministic inbound policy evaluation.
#[derive(Debug, Clone)]
pub enum PolicyDecision {
    Accept(Box<MemoryItem>),
    Duplicate(MemoryReceipt),
}

/// Deterministic fast-path policy router: schema validation and idempotency deduplication.
#[derive(Default)]
pub struct PolicyRouter {
    seen_keys: RwLock<HashMap<(ProjectId, String), MemoryReceipt>>,
}

impl PolicyRouter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn evaluate(&self, input: &MemoryInput) -> Result<PolicyDecision> {
        input.validate()?;

        if let Some(key) = &input.idempotency_key {
            let map = self.seen_keys.read().unwrap();
            if let Some(existing) = map.get(&(input.project_id.clone(), key.clone())) {
                return Ok(PolicyDecision::Duplicate(existing.clone()));
            }
        }

        let item = MemoryItem::from_input(input.clone(), 1);
        if let Some(key) = &item.idempotency_key {
            let mut map = self.seen_keys.write().unwrap();
            if let Some(existing) = map.get(&(item.project_id.clone(), key.clone())) {
                return Ok(PolicyDecision::Duplicate(existing.clone()));
            }
            map.insert(
                (item.project_id.clone(), key.clone()),
                MemoryReceipt {
                    memory_id: item.id,
                    state: ReceiptState::Buffered,
                    version: item.version,
                },
            );
        }

        Ok(PolicyDecision::Accept(Box::new(item)))
    }
}

/// Bounded per-project in-memory cache of recently written/updated `MemoryItem`s.
pub struct HotContextCache {
    per_project_capacity: usize,
    entries: RwLock<HashMap<ProjectId, VecDeque<MemoryItem>>>,
}

impl HotContextCache {
    pub fn new(per_project_capacity: usize) -> Self {
        Self {
            per_project_capacity: per_project_capacity.max(1),
            entries: RwLock::new(HashMap::new()),
        }
    }

    pub fn insert(&self, item: MemoryItem) {
        let mut guard = self.entries.write().unwrap();
        let queue = guard.entry(item.project_id.clone()).or_default();
        queue.retain(|existing| existing.id != item.id);
        queue.push_front(item);
        while queue.len() > self.per_project_capacity {
            queue.pop_back();
        }
    }

    pub fn get(&self, project_id: &ProjectId, memory_id: MemoryId) -> Option<MemoryItem> {
        let guard = self.entries.read().unwrap();
        guard
            .get(project_id)
            .and_then(|q| q.iter().find(|item| item.id == memory_id).cloned())
    }

    pub fn remove(&self, project_id: &ProjectId, memory_id: MemoryId) {
        let mut guard = self.entries.write().unwrap();
        if let Some(queue) = guard.get_mut(project_id) {
            queue.retain(|item| item.id != memory_id);
        }
    }

    pub fn recent(&self, project_id: &ProjectId, limit: usize) -> Vec<MemoryItem> {
        let guard = self.entries.read().unwrap();
        guard
            .get(project_id)
            .map(|q| q.iter().take(limit).cloned().collect())
            .unwrap_or_default()
    }
}

/// Write-behind persistence buffer coordinating asynchronous/barriered flushes to outbound stores.
pub struct IngestionBuffer {
    vector_store: Arc<dyn VectorStore>,
    graph_store: Arc<dyn GraphStore>,
    pending: Mutex<VecDeque<MemoryItem>>,
    flush_lock: tokio::sync::Mutex<()>,
    closed: RwLock<bool>,
}

impl IngestionBuffer {
    pub fn new(vector_store: Arc<dyn VectorStore>, graph_store: Arc<dyn GraphStore>) -> Self {
        Self {
            vector_store,
            graph_store,
            pending: Mutex::new(VecDeque::new()),
            flush_lock: tokio::sync::Mutex::new(()),
            closed: RwLock::new(false),
        }
    }

    pub async fn enqueue(
        &self,
        item: MemoryItem,
        durability: WriteDurability,
    ) -> Result<MemoryReceipt> {
        let span = tracing::info_span!(
            "aether.cortex.enqueue",
            project_id = %item.project_id,
            memory_id = %item.id,
            durability = ?durability,
        );
        async move {
            if *self.closed.read().unwrap() {
                return Err(AetherError::InvalidInput(
                    "ingestion buffer is shut down".into(),
                ));
            }
            let receipt_id = item.id;
            let version = item.version;
            {
                let mut queue = self.pending.lock().unwrap();
                queue.retain(|existing| existing.id != item.id);
                queue.push_back(item);
            }

            match durability {
                WriteDurability::Buffered => Ok(MemoryReceipt {
                    memory_id: receipt_id,
                    state: ReceiptState::Buffered,
                    version,
                }),
                WriteDurability::WaitFlush => {
                    self.flush().await?;
                    Ok(MemoryReceipt {
                        memory_id: receipt_id,
                        state: ReceiptState::Persisted,
                        version,
                    })
                }
            }
        }
        .instrument(span)
        .await
    }

    pub fn pending_snapshot(&self, project_id: &ProjectId) -> Vec<MemoryItem> {
        let queue = self.pending.lock().unwrap();
        queue
            .iter()
            .filter(|item| &item.project_id == project_id)
            .cloned()
            .collect()
    }

    pub fn remove_pending(&self, project_id: &ProjectId, memory_id: MemoryId) {
        let mut queue = self.pending.lock().unwrap();
        queue.retain(|item| !(&item.project_id == project_id && item.id == memory_id));
    }

    /// Drains all buffered writes to the configured `VectorStore` and `GraphStore`.
    pub async fn flush(&self) -> Result<()> {
        let span = tracing::info_span!("aether.cortex.flush", item_count = tracing::field::Empty,);
        async {
            let _guard = self.flush_lock.lock().await;
            let items: Vec<MemoryItem> = {
                let mut queue = self.pending.lock().unwrap();
                queue.drain(..).collect()
            };
            tracing::Span::current().record("item_count", items.len());

            if items.is_empty() {
                return Ok(());
            }

            let mut by_project: HashMap<ProjectId, Vec<MemoryItem>> = HashMap::new();
            for item in items {
                by_project
                    .entry(item.project_id.clone())
                    .or_default()
                    .push(item);
            }

            for (project_id, project_items) in by_project {
                let vector_records: Vec<VectorRecord> = project_items
                    .iter()
                    .filter_map(|item| {
                        item.embedding.as_ref().map(|vec| VectorRecord {
                            id: item.id,
                            vector: vec.clone(),
                            metadata: enrich_metadata(item),
                        })
                    })
                    .collect();

                if !vector_records.is_empty() {
                    self.vector_store
                        .upsert_batch(&project_id, &vector_records)
                        .await?;
                }

                for item in &project_items {
                    if !item.entities.is_empty() {
                        self.graph_store
                            .upsert_entities(&project_id, &item.entities)
                            .await?;
                    }
                    if !item.relations.is_empty() {
                        self.graph_store
                            .upsert_relations(&project_id, item.id, &item.relations)
                            .await?;
                    }
                }
            }

            Ok(())
        }
        .instrument(span)
        .await
    }

    pub async fn shutdown(&self) -> Result<()> {
        *self.closed.write().unwrap() = true;
        self.flush().await?;
        self.vector_store.close().await?;
        self.graph_store.close().await?;
        Ok(())
    }
}

fn enrich_metadata(item: &MemoryItem) -> Value {
    let mut meta = match &item.metadata {
        Value::Object(map) => map.clone(),
        _ => serde_json::Map::new(),
    };
    meta.insert("content".into(), Value::String(item.content.clone()));
    meta.insert("version".into(), Value::Number(item.version.into()));
    Value::Object(meta)
}

/// Plans and executes Vector, Graph, and Hybrid recall with L1/L2 Read-Your-Writes merging.
pub struct QueryPlanner {
    record_store: Arc<dyn RecordStore>,
    vector_store: Arc<dyn VectorStore>,
    graph_store: Arc<dyn GraphStore>,
    hot_cache: Arc<HotContextCache>,
    buffer: Arc<IngestionBuffer>,
    ppr: PersonalizedPageRank,
}

impl QueryPlanner {
    pub fn new(
        record_store: Arc<dyn RecordStore>,
        vector_store: Arc<dyn VectorStore>,
        graph_store: Arc<dyn GraphStore>,
        hot_cache: Arc<HotContextCache>,
        buffer: Arc<IngestionBuffer>,
    ) -> Self {
        Self {
            record_store,
            vector_store,
            graph_store,
            hot_cache,
            buffer,
            ppr: PersonalizedPageRank::default(),
        }
    }

    pub async fn plan_and_execute(&self, query: &RecallQuery) -> Result<RecallResult> {
        let span = tracing::info_span!(
            "aether.cortex.query_plan",
            project_id = %query.project_id,
            mode = ?query.mode,
            consistency = ?query.consistency,
            top_k = query.top_k,
        );
        self.plan_and_execute_inner(query).instrument(span).await
    }

    async fn plan_and_execute_inner(&self, query: &RecallQuery) -> Result<RecallResult> {
        let mut hits_by_id: HashMap<MemoryId, RecallHit> = HashMap::new();

        if matches!(query.mode, RecallMode::Vector | RecallMode::Hybrid) {
            if let Some(qvec) = &query.query_vector {
                let matches = self
                    .vector_store
                    .search(&query.project_id, qvec, query.top_k, query.filters.as_ref())
                    .await?;
                for m in matches {
                    let content = m
                        .metadata
                        .get("content")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    hits_by_id.insert(
                        m.id,
                        RecallHit {
                            memory_id: m.id,
                            content,
                            score: m.score,
                            metadata: m.metadata,
                        },
                    );
                }
            }
        }

        // Read-Your-Writes: merge buffered, L1 hot-cache, and L2 RecordStore items.
        if query.consistency == ConsistencyMode::ReadYourWrites {
            let pending = self.buffer.pending_snapshot(&query.project_id);
            let hot_recent = self.hot_cache.recent(&query.project_id, query.top_k);
            let record_recent = self
                .record_store
                .recent(&query.project_id, None, query.top_k)
                .await?;
            for item in pending.into_iter().chain(hot_recent).chain(record_recent) {
                if hits_by_id.contains_key(&item.id) {
                    continue;
                }
                let meta = enrich_metadata(&item);
                if let Some(filter) = &query.filters {
                    if !filter.matches(&meta) {
                        continue;
                    }
                }
                let score = match (&query.query_vector, &item.embedding) {
                    (Some(q), Some(v)) if q.len() == v.len() => DistanceMetric::Cosine.score(q, v),
                    _ if item
                        .content
                        .to_lowercase()
                        .contains(&query.query.to_lowercase()) =>
                    {
                        0.75
                    }
                    _ => 0.5,
                };
                hits_by_id.insert(
                    item.id,
                    RecallHit {
                        memory_id: item.id,
                        content: item.content,
                        score,
                        metadata: meta,
                    },
                );
            }
        }

        let subgraph = if matches!(query.mode, RecallMode::Graph | RecallMode::Hybrid) {
            let seeds = self.resolve_seeds(query);
            if seeds.is_empty() {
                Subgraph::default()
            } else {
                let sub = self
                    .graph_store
                    .neighborhood(
                        &query.project_id,
                        &seeds,
                        query.hops,
                        query.per_node_limit,
                        &query.temporal,
                    )
                    .await?;
                let _ranked = self.ppr.rank(&sub, &seeds);
                sub
            }
        } else {
            Subgraph::default()
        };

        let mut hits: Vec<RecallHit> = hits_by_id.into_values().collect();
        hits.sort_by(|a, b| b.score.total_cmp(&a.score));
        hits.truncate(query.top_k);

        Ok(RecallResult { hits, subgraph })
    }

    fn resolve_seeds(&self, query: &RecallQuery) -> Vec<EntityId> {
        if !query.seed_entities.is_empty() {
            return query.seed_entities.clone();
        }
        let q_lower = query.query.to_lowercase();
        let mut seeds = Vec::new();
        for item in self
            .buffer
            .pending_snapshot(&query.project_id)
            .into_iter()
            .chain(self.hot_cache.recent(&query.project_id, 25))
        {
            for entity in item.entities {
                if (q_lower.contains(&entity.name.to_lowercase())
                    || q_lower.contains(&entity.id.as_str().to_lowercase()))
                    && !seeds.contains(&entity.id)
                {
                    seeds.push(entity.id);
                }
            }
        }
        seeds
    }
}
