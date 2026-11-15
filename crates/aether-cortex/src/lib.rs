//! Gateway core orchestration for Aether SDK:
//! `PolicyRouter`, `HotContextCache`, `IngestionBuffer`, `QueryPlanner`, and `PersonalizedPageRank`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex, RwLock};

use aether_core::{
    AetherError, ConsistencyMode, EntityId, FilterExpr, FilterValue, GraphStore, KvStore, MemoryId,
    MemoryInput, MemoryItem, MemoryReceipt, ProjectId, ReceiptState, RecallHit, RecallMode,
    RecallQuery, RecallResult, Result, Subgraph, VectorRecord, VectorStore, WriteDurability,
};
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::EdgeRef;
use serde_json::Value;

/// Result of deterministic inbound policy evaluation.
#[derive(Debug, Clone)]
pub enum PolicyDecision {
    Accept(MemoryItem),
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

        Ok(PolicyDecision::Accept(item))
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
        let _guard = self.flush_lock.lock().await;
        let items: Vec<MemoryItem> = {
            let mut queue = self.pending.lock().unwrap();
            queue.drain(..).collect()
        };

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

/// Personalized PageRank over a retrieved `Subgraph` using `petgraph`.
pub struct PersonalizedPageRank {
    pub damping: f32,
    pub iterations: usize,
}

impl Default for PersonalizedPageRank {
    fn default() -> Self {
        Self {
            damping: 0.85,
            iterations: 20,
        }
    }
}

impl PersonalizedPageRank {
    pub fn rank(&self, subgraph: &Subgraph, seeds: &[EntityId]) -> Vec<(EntityId, f32)> {
        if subgraph.entities.is_empty() {
            return Vec::new();
        }

        let mut graph = DiGraph::<EntityId, ()>::new();
        let mut indices: HashMap<EntityId, NodeIndex> = HashMap::new();
        for entity in &subgraph.entities {
            let idx = graph.add_node(entity.id.clone());
            indices.insert(entity.id.clone(), idx);
        }

        for edge in &subgraph.edges {
            if let (Some(&src), Some(&dst)) = (indices.get(&edge.source), indices.get(&edge.target)) {
                graph.add_edge(src, dst, ());
            }
        }

        let n = graph.node_count();
        let seed_set: HashSet<&EntityId> = seeds.iter().collect();
        let mut personalization = vec![0.0f32; n];
        let seed_hits: usize = graph
            .node_indices()
            .filter(|&idx| seed_set.contains(&graph[idx]))
            .count();

        if seed_hits > 0 {
            let weight = 1.0 / seed_hits as f32;
            for idx in graph.node_indices() {
                if seed_set.contains(&graph[idx]) {
                    personalization[idx.index()] = weight;
                }
            }
        } else {
            personalization.fill(1.0 / n as f32);
        }

        let mut scores = personalization.clone();
        for _ in 0..self.iterations {
            let mut next = vec![0.0f32; n];
            for idx in graph.node_indices() {
                let out_deg = graph.edges(idx).count();
                if out_deg == 0 {
                    for (j, p) in personalization.iter().enumerate() {
                        next[j] += self.damping * scores[idx.index()] * p;
                    }
                } else {
                    let share = self.damping * scores[idx.index()] / out_deg as f32;
                    for edge in graph.edges(idx) {
                        next[edge.target().index()] += share;
                    }
                }
            }
            for (j, p) in personalization.iter().enumerate() {
                next[j] += (1.0 - self.damping) * p;
            }
            scores = next;
        }

        let mut ranked: Vec<(EntityId, f32)> = graph
            .node_indices()
            .map(|idx| (graph[idx].clone(), scores[idx.index()]))
            .collect();
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
        ranked
    }
}

/// Plans and executes Vector, Graph, and Hybrid recall with L1/L2 Read-Your-Writes merging.
pub struct QueryPlanner {
    kv_store: Arc<dyn KvStore>,
    vector_store: Arc<dyn VectorStore>,
    graph_store: Arc<dyn GraphStore>,
    hot_cache: Arc<HotContextCache>,
    buffer: Arc<IngestionBuffer>,
    ppr: PersonalizedPageRank,
}

impl QueryPlanner {
    pub fn new(
        kv_store: Arc<dyn KvStore>,
        vector_store: Arc<dyn VectorStore>,
        graph_store: Arc<dyn GraphStore>,
        hot_cache: Arc<HotContextCache>,
        buffer: Arc<IngestionBuffer>,
    ) -> Self {
        Self {
            kv_store,
            vector_store,
            graph_store,
            hot_cache,
            buffer,
            ppr: PersonalizedPageRank::default(),
        }
    }

    pub async fn plan_and_execute(&self, query: &RecallQuery) -> Result<RecallResult> {
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

        // Read-Your-Writes: merge buffered, L1 hot-cache, and L2 KvStore items.
        if query.consistency == ConsistencyMode::ReadYourWrites {
            let pending = self.buffer.pending_snapshot(&query.project_id);
            let hot_recent = self.hot_cache.recent(&query.project_id, query.top_k);
            let kv_recent = self
                .kv_store
                .recent(&query.project_id, None, query.top_k)
                .await?;
            for item in pending
                .into_iter()
                .chain(hot_recent.into_iter())
                .chain(kv_recent.into_iter())
            {
                if hits_by_id.contains_key(&item.id) {
                    continue;
                }
                let meta = enrich_metadata(&item);
                if let Some(filter) = &query.filters {
                    if !eval_filter(filter, &meta) {
                        continue;
                    }
                }
                let score = match (&query.query_vector, &item.embedding) {
                    (Some(q), Some(v)) if q.len() == v.len() => cosine(q, v),
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
                if q_lower.contains(&entity.name.to_lowercase())
                    || q_lower.contains(&entity.id.as_str().to_lowercase())
                {
                    if !seeds.contains(&entity.id) {
                        seeds.push(entity.id);
                    }
                }
            }
        }
        seeds
    }
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na * nb)
    }
}

fn eval_filter(expr: &FilterExpr, metadata: &Value) -> bool {
    match expr {
        FilterExpr::Eq { field, value } => metadata
            .get(field)
            .map_or(false, |v| value_matches(v, value)),
        FilterExpr::In { field, values } => metadata
            .get(field)
            .map_or(false, |v| values.iter().any(|target| value_matches(v, target))),
        FilterExpr::Range { field, gte, lte } => metadata
            .get(field)
            .and_then(|v| v.as_f64())
            .map_or(false, |num| {
                gte.map_or(true, |min| num >= min) && lte.map_or(true, |max| num <= max)
            }),
        FilterExpr::And(exprs) => exprs.iter().all(|e| eval_filter(e, metadata)),
        FilterExpr::Or(exprs) => exprs.iter().any(|e| eval_filter(e, metadata)),
    }
}

fn value_matches(json_val: &Value, filter_val: &FilterValue) -> bool {
    match (json_val, filter_val) {
        (Value::String(a), FilterValue::String(b)) => a == b,
        (Value::Number(a), FilterValue::Int(b)) => a.as_i64() == Some(*b),
        (Value::Number(a), FilterValue::Float(b)) => {
            a.as_f64().map_or(false, |x| (x - b).abs() < f64::EPSILON)
        }
        (Value::Bool(a), FilterValue::Bool(b)) => a == b,
        _ => false,
    }
}
