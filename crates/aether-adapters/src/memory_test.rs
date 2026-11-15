//! In-memory `VectorStore` + `petgraph` `GraphStore` for `cargo test` and local dry-runs.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::RwLock;
use std::time::{Duration, Instant};

use aether_core::{
    AetherError, ConnectorCapabilities, ConnectorHealth, DistanceMetric, Entity, EntityId,
    FilterExpr, FilterValue, GraphEdge, GraphStore, KvStore, MemoryId, MemoryItem, MemoryReceipt,
    MemoryScope, OutboundConnector, ProjectId, Relation, Result, Subgraph, TemporalFilter,
    VectorMatch, VectorRecord, VectorStore,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::EdgeRef;
use petgraph::Direction;
use serde_json::Value;

#[derive(Default)]
struct VectorNamespace {
    dimensions: Option<usize>,
    metric: DistanceMetric,
    records: HashMap<MemoryId, VectorRecord>,
}

#[derive(Default)]
struct GraphNamespace {
    graph: DiGraph<Entity, GraphEdge>,
    node_index: HashMap<EntityId, NodeIndex>,
}

struct KvEntry {
    item: MemoryItem,
    expires_at: Option<Instant>,
    seq: u64,
}

struct IdemEntry {
    receipt: MemoryReceipt,
    expires_at: Option<Instant>,
}

#[derive(Default)]
struct KvNamespace {
    items: HashMap<MemoryId, KvEntry>,
    idempotency: HashMap<String, IdemEntry>,
    next_seq: u64,
}

/// Zero-infra, thread-safe in-memory backend implementing `KvStore`, `VectorStore`, and `GraphStore`.
#[derive(Default)]
pub struct InMemoryBackend {
    kv: RwLock<HashMap<ProjectId, KvNamespace>>,
    vectors: RwLock<HashMap<ProjectId, VectorNamespace>>,
    graphs: RwLock<HashMap<ProjectId, GraphNamespace>>,
}

impl InMemoryBackend {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl OutboundConnector for InMemoryBackend {
    fn name(&self) -> &'static str {
        "in_memory"
    }

    fn capabilities(&self) -> ConnectorCapabilities {
        ConnectorCapabilities {
            max_batch_size: 1024,
            supports_temporal: true,
            supports_filter_expr: true,
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
impl KvStore for InMemoryBackend {
    async fn get(&self, ns: &ProjectId, id: MemoryId) -> Result<Option<MemoryItem>> {
        let now = Instant::now();
        let guard = self.kv.read().unwrap();
        let Some(entry) = guard.get(ns) else {
            return Ok(None);
        };
        Ok(entry.items.get(&id).and_then(|e| {
            if e.expires_at.map_or(false, |exp| now >= exp) {
                None
            } else {
                Some(e.item.clone())
            }
        }))
    }

    async fn put(&self, ns: &ProjectId, item: &MemoryItem, ttl: Option<Duration>) -> Result<()> {
        let now = Instant::now();
        let mut guard = self.kv.write().unwrap();
        let entry = guard.entry(ns.clone()).or_default();
        entry.next_seq += 1;
        let seq = entry.next_seq;
        entry.items.insert(
            item.id,
            KvEntry {
                item: item.clone(),
                expires_at: ttl.map(|d| now + d),
                seq,
            },
        );
        Ok(())
    }

    async fn put_if_absent(
        &self,
        ns: &ProjectId,
        idempotency_key: &str,
        receipt: &MemoryReceipt,
        ttl: Option<Duration>,
    ) -> Result<Option<MemoryReceipt>> {
        let now = Instant::now();
        let mut guard = self.kv.write().unwrap();
        let entry = guard.entry(ns.clone()).or_default();
        if let Some(existing) = entry.idempotency.get(idempotency_key) {
            if !existing.expires_at.map_or(false, |exp| now >= exp) {
                return Ok(Some(existing.receipt.clone()));
            }
        }
        entry.idempotency.insert(
            idempotency_key.to_string(),
            IdemEntry {
                receipt: receipt.clone(),
                expires_at: ttl.map(|d| now + d),
            },
        );
        Ok(None)
    }

    async fn cas_update(
        &self,
        ns: &ProjectId,
        id: MemoryId,
        expected_version: Option<u64>,
        new_item: &MemoryItem,
    ) -> Result<()> {
        let now = Instant::now();
        let mut guard = self.kv.write().unwrap();
        let entry = guard
            .get_mut(ns)
            .ok_or_else(|| AetherError::NotFound(format!("memory {id}")))?;
        let current = entry
            .items
            .get(&id)
            .filter(|e| !e.expires_at.map_or(false, |exp| now >= exp))
            .ok_or_else(|| AetherError::NotFound(format!("memory {id}")))?;

        if let Some(expected) = expected_version {
            if current.item.version != expected {
                return Err(AetherError::Conflict {
                    memory_id: id,
                    expected_version: expected,
                    actual_version: current.item.version,
                });
            }
        }

        let expires_at = current.expires_at;
        entry.next_seq += 1;
        let seq = entry.next_seq;
        entry.items.insert(
            id,
            KvEntry {
                item: new_item.clone(),
                expires_at,
                seq,
            },
        );
        Ok(())
    }

    async fn recent(
        &self,
        ns: &ProjectId,
        scope: Option<MemoryScope>,
        limit: usize,
    ) -> Result<Vec<MemoryItem>> {
        let now = Instant::now();
        let guard = self.kv.read().unwrap();
        let Some(entry) = guard.get(ns) else {
            return Ok(Vec::new());
        };
        let mut items: Vec<&KvEntry> = entry
            .items
            .values()
            .filter(|e| !e.expires_at.map_or(false, |exp| now >= exp))
            .filter(|e| scope.map_or(true, |s| e.item.scope == s))
            .collect();
        items.sort_by(|a, b| b.seq.cmp(&a.seq));
        Ok(items
            .into_iter()
            .take(limit)
            .map(|e| e.item.clone())
            .collect())
    }

    async fn delete(&self, ns: &ProjectId, id: MemoryId) -> Result<()> {
        let mut guard = self.kv.write().unwrap();
        if let Some(entry) = guard.get_mut(ns) {
            entry.items.remove(&id);
        }
        Ok(())
    }
}

#[async_trait]
impl VectorStore for InMemoryBackend {
    async fn initialize(
        &self,
        ns: &ProjectId,
        dimensions: usize,
        metric: DistanceMetric,
    ) -> Result<()> {
        if dimensions == 0 {
            return Err(AetherError::InvalidInput(
                "vector dimensions must be greater than 0".into(),
            ));
        }
        let mut guard = self.vectors.write().unwrap();
        let entry = guard.entry(ns.clone()).or_default();
        entry.dimensions = Some(dimensions);
        entry.metric = metric;
        Ok(())
    }

    async fn upsert_batch(&self, ns: &ProjectId, records: &[VectorRecord]) -> Result<()> {
        let mut guard = self.vectors.write().unwrap();
        let entry = guard.entry(ns.clone()).or_default();
        for rec in records {
            if let Some(expected_dim) = entry.dimensions {
                if rec.vector.len() != expected_dim {
                    return Err(AetherError::InvalidInput(format!(
                        "vector dimension mismatch: expected {expected_dim}, got {}",
                        rec.vector.len()
                    )));
                }
            }
            entry.records.insert(rec.id, rec.clone());
        }
        Ok(())
    }

    async fn delete(&self, ns: &ProjectId, ids: &[MemoryId]) -> Result<()> {
        let mut guard = self.vectors.write().unwrap();
        if let Some(entry) = guard.get_mut(ns) {
            for id in ids {
                entry.records.remove(id);
            }
        }
        Ok(())
    }

    async fn search(
        &self,
        ns: &ProjectId,
        query_vector: &[f32],
        top_k: usize,
        filter: Option<&FilterExpr>,
    ) -> Result<Vec<VectorMatch>> {
        let guard = self.vectors.read().unwrap();
        let Some(entry) = guard.get(ns) else {
            return Ok(Vec::new());
        };
        if let Some(expected_dim) = entry.dimensions {
            if query_vector.len() != expected_dim {
                return Err(AetherError::InvalidInput(format!(
                    "query vector dimension mismatch: expected {expected_dim}, got {}",
                    query_vector.len()
                )));
            }
        }

        let mut scored: Vec<VectorMatch> = entry
            .records
            .values()
            .filter(|rec| match filter {
                Some(expr) => eval_filter(expr, &rec.metadata),
                None => true,
            })
            .map(|rec| VectorMatch {
                id: rec.id,
                score: similarity(entry.metric, query_vector, &rec.vector),
                metadata: rec.metadata.clone(),
            })
            .collect();

        scored.sort_by(|a, b| b.score.total_cmp(&a.score));
        scored.truncate(top_k);
        Ok(scored)
    }
}

#[async_trait]
impl GraphStore for InMemoryBackend {
    async fn upsert_entities(&self, ns: &ProjectId, entities: &[Entity]) -> Result<()> {
        let mut guard = self.graphs.write().unwrap();
        let entry = guard.entry(ns.clone()).or_default();
        for entity in entities {
            if let Some(&idx) = entry.node_index.get(&entity.id) {
                entry.graph[idx] = entity.clone();
            } else {
                let idx = entry.graph.add_node(entity.clone());
                entry.node_index.insert(entity.id.clone(), idx);
            }
        }
        Ok(())
    }

    async fn upsert_relations(
        &self,
        ns: &ProjectId,
        source_memory_id: MemoryId,
        relations: &[Relation],
    ) -> Result<()> {
        let mut guard = self.graphs.write().unwrap();
        let entry = guard.entry(ns.clone()).or_default();
        for rel in relations {
            let src_idx = ensure_node(entry, &rel.source);
            let dst_idx = ensure_node(entry, &rel.target);
            entry.graph.add_edge(
                src_idx,
                dst_idx,
                GraphEdge {
                    source: rel.source.clone(),
                    target: rel.target.clone(),
                    relation_type: rel.relation_type.clone(),
                    source_memory_id,
                    valid_at: rel.valid_at,
                    invalid_at: rel.invalid_at,
                },
            );
        }
        Ok(())
    }

    async fn retract_memory_facts(
        &self,
        ns: &ProjectId,
        source_memory_id: MemoryId,
        invalidated_at: DateTime<Utc>,
    ) -> Result<()> {
        let mut guard = self.graphs.write().unwrap();
        if let Some(entry) = guard.get_mut(ns) {
            for edge in entry.graph.edge_weights_mut() {
                if edge.source_memory_id == source_memory_id && edge.invalid_at.is_none() {
                    edge.invalid_at = Some(invalidated_at);
                }
            }
        }
        Ok(())
    }

    async fn neighborhood(
        &self,
        ns: &ProjectId,
        seeds: &[EntityId],
        hops: u8,
        per_node_limit: usize,
        temporal: &TemporalFilter,
    ) -> Result<Subgraph> {
        let guard = self.graphs.read().unwrap();
        let Some(entry) = guard.get(ns) else {
            return Ok(Subgraph::default());
        };

        let mut visited_nodes: HashSet<NodeIndex> = HashSet::new();
        let mut visited_edges: HashSet<petgraph::graph::EdgeIndex> = HashSet::new();
        let mut queue: VecDeque<(NodeIndex, u8)> = VecDeque::new();

        for seed in seeds {
            if let Some(&idx) = entry.node_index.get(seed) {
                if visited_nodes.insert(idx) {
                    queue.push_back((idx, 0));
                }
            }
        }

        let mut collected_edges = Vec::new();
        while let Some((node_idx, depth)) = queue.pop_front() {
            if depth >= hops {
                continue;
            }
            let mut per_node_count = 0usize;
            for dir in [Direction::Outgoing, Direction::Incoming] {
                for edge_ref in entry.graph.edges_directed(node_idx, dir) {
                    if per_node_count >= per_node_limit {
                        break;
                    }
                    let edge = edge_ref.weight();
                    if !matches_temporal(edge, temporal) || !visited_edges.insert(edge_ref.id()) {
                        continue;
                    }
                    collected_edges.push(edge.clone());
                    per_node_count += 1;

                    let neighbor = match dir {
                        Direction::Outgoing => edge_ref.target(),
                        Direction::Incoming => edge_ref.source(),
                    };
                    if visited_nodes.insert(neighbor) {
                        queue.push_back((neighbor, depth + 1));
                    }
                }
            }
        }

        let entities = visited_nodes
            .into_iter()
            .map(|idx| entry.graph[idx].clone())
            .collect();

        Ok(Subgraph {
            entities,
            edges: collected_edges,
        })
    }
}

fn ensure_node(ns: &mut GraphNamespace, id: &EntityId) -> NodeIndex {
    if let Some(&idx) = ns.node_index.get(id) {
        return idx;
    }
    let idx = ns.graph.add_node(Entity {
        id: id.clone(),
        name: id.as_str().to_string(),
        entity_type: "Unknown".into(),
    });
    ns.node_index.insert(id.clone(), idx);
    idx
}

fn matches_temporal(edge: &GraphEdge, temporal: &TemporalFilter) -> bool {
    if let Some(as_of) = temporal.as_of {
        let valid = edge.valid_at <= as_of;
        let not_invalid = edge.invalid_at.map_or(true, |inv| as_of < inv);
        if !(valid && not_invalid) {
            return false;
        }
    }
    if temporal.active_only && edge.invalid_at.is_some() {
        return false;
    }
    true
}

fn similarity(metric: DistanceMetric, a: &[f32], b: &[f32]) -> f32 {
    match metric {
        DistanceMetric::Dot => a.iter().zip(b).map(|(x, y)| x * y).sum(),
        DistanceMetric::Cosine => {
            let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
            let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
            let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
            if norm_a == 0.0 || norm_b == 0.0 {
                0.0
            } else {
                dot / (norm_a * norm_b)
            }
        }
        DistanceMetric::Euclidean => {
            let dist_sq: f32 = a.iter().zip(b).map(|(x, y)| (x - y).powi(2)).sum();
            -dist_sq.sqrt()
        }
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
        FilterExpr::Range { field, gte, lte } => metadata.get(field).and_then(|v| v.as_f64()).map_or(false, |num| {
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
