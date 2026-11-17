//! In-memory `VectorStore` + `petgraph` `GraphStore` for `cargo test` and local dry-runs.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, RwLock};
use std::time::{Duration, Instant};

use aether_core::{
    AetherError, ConnectorCapabilities, ConnectorHealth, CreateOutcome, DistanceMetric, Entity,
    EntityId, FilterExpr, GraphEdge, GraphStore, IndexSpec, MemoryId, MemoryItem, MemoryReceipt,
    MemoryScope, OutboundConnector, PendingRecord, ProjectId, ReceiptState, RecordStore, Relation,
    Result, SessionId, Subgraph, TemporalFilter, VectorMatch, VectorRecord, VectorStore,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::EdgeRef;
use petgraph::Direction;

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

struct RecordEntry {
    item: MemoryItem,
    expires_at: Option<Instant>,
    seq: u64,
    indexed_version: u64,
    tombstone: bool,
}

struct IdemEntry {
    receipt: MemoryReceipt,
    expires_at: Option<Instant>,
}

#[derive(Default)]
struct RecordNamespace {
    items: HashMap<MemoryId, RecordEntry>,
    idempotency: HashMap<String, IdemEntry>,
    create_keys: HashMap<String, MemoryId>,
    next_seq: u64,
}

/// Operations on [`InMemoryBackend`] that can be configured to fail transiently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FaultPoint {
    VectorUpsert,
    VectorDelete,
    GraphUpsert,
    GraphRetract,
}

/// Zero-infra, thread-safe in-memory backend implementing `RecordStore`, `VectorStore`, and `GraphStore`.
#[derive(Default)]
pub struct InMemoryBackend {
    records: RwLock<HashMap<ProjectId, RecordNamespace>>,
    vectors: RwLock<HashMap<ProjectId, VectorNamespace>>,
    graphs: RwLock<HashMap<ProjectId, GraphNamespace>>,
    faults: Mutex<HashMap<FaultPoint, usize>>,
    global_seq: AtomicU64,
}

impl InMemoryBackend {
    pub fn new() -> Self {
        Self::default()
    }

    /// Injects `times` transient faults for the given `point` on subsequent calls.
    pub fn fail_next(&self, point: FaultPoint, times: usize) {
        let mut faults = self.faults.lock().unwrap();
        if times == 0 {
            faults.remove(&point);
        } else {
            faults.insert(point, times);
        }
    }

    /// Returns whether a vector with `id` currently exists in namespace `ns`.
    pub fn has_vector(&self, ns: &ProjectId, id: MemoryId) -> bool {
        let guard = self.vectors.read().unwrap();
        guard
            .get(ns)
            .is_some_and(|entry| entry.records.contains_key(&id))
    }

    fn maybe_inject_fault(&self, point: FaultPoint) -> Result<()> {
        let mut faults = self.faults.lock().unwrap();
        if let Some(remaining) = faults.get_mut(&point) {
            if *remaining > 0 {
                *remaining -= 1;
                if *remaining == 0 {
                    faults.remove(&point);
                }
                return Err(AetherError::Transient {
                    connector: "in_memory",
                    message: "injected fault".into(),
                });
            }
        }
        Ok(())
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
impl RecordStore for InMemoryBackend {
    async fn create(
        &self,
        ns: &ProjectId,
        item: &MemoryItem,
        ttl: Option<Duration>,
    ) -> Result<CreateOutcome> {
        let now = Instant::now();
        let mut guard = self.records.write().unwrap();
        let entry = guard.entry(ns.clone()).or_default();

        if let Some(key) = &item.idempotency_key {
            if let Some(&existing_id) = entry.create_keys.get(key) {
                if let Some(existing) = entry.items.get(&existing_id) {
                    let live =
                        !existing.tombstone && existing.expires_at.is_none_or(|exp| now < exp);
                    if live {
                        let state = if existing.indexed_version == existing.item.version {
                            ReceiptState::Persisted
                        } else {
                            ReceiptState::Buffered
                        };
                        return Ok(CreateOutcome::Duplicate(MemoryReceipt {
                            memory_id: existing.item.id,
                            state,
                            version: existing.item.version,
                        }));
                    }
                }
            }
            entry.create_keys.insert(key.clone(), item.id);
        }

        let gseq = self.global_seq.fetch_add(1, Ordering::SeqCst) + 1;
        entry.next_seq = gseq;
        entry.items.insert(
            item.id,
            RecordEntry {
                item: item.clone(),
                expires_at: ttl.map(|d| now + d),
                seq: gseq,
                indexed_version: 0,
                tombstone: false,
            },
        );
        Ok(CreateOutcome::Created)
    }

    async fn get(&self, ns: &ProjectId, id: MemoryId) -> Result<Option<MemoryItem>> {
        let now = Instant::now();
        let guard = self.records.read().unwrap();
        let Some(entry) = guard.get(ns) else {
            return Ok(None);
        };
        Ok(entry.items.get(&id).and_then(|e| {
            if e.tombstone || e.expires_at.is_some_and(|exp| now >= exp) {
                None
            } else {
                Some(e.item.clone())
            }
        }))
    }

    async fn get_many(&self, ns: &ProjectId, ids: &[MemoryId]) -> Result<Vec<MemoryItem>> {
        let now = Instant::now();
        let guard = self.records.read().unwrap();
        let Some(entry) = guard.get(ns) else {
            return Ok(Vec::new());
        };
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(e) = entry.items.get(id) {
                if !e.tombstone && e.expires_at.is_none_or(|exp| now < exp) {
                    out.push(e.item.clone());
                }
            }
        }
        Ok(out)
    }

    async fn put(&self, ns: &ProjectId, item: &MemoryItem, ttl: Option<Duration>) -> Result<()> {
        let now = Instant::now();
        let mut guard = self.records.write().unwrap();
        let entry = guard.entry(ns.clone()).or_default();
        let gseq = self.global_seq.fetch_add(1, Ordering::SeqCst) + 1;
        entry.next_seq = gseq;
        entry.items.insert(
            item.id,
            RecordEntry {
                item: item.clone(),
                expires_at: ttl.map(|d| now + d),
                seq: gseq,
                indexed_version: 0,
                tombstone: false,
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
        let mut guard = self.records.write().unwrap();
        let entry = guard.entry(ns.clone()).or_default();
        if let Some(existing) = entry.idempotency.get(idempotency_key) {
            if existing.expires_at.is_none_or(|exp| now < exp) {
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
        let mut guard = self.records.write().unwrap();
        let entry = guard
            .get_mut(ns)
            .ok_or_else(|| AetherError::NotFound(format!("memory {id}")))?;
        let current = entry
            .items
            .get(&id)
            .filter(|e| !e.tombstone && e.expires_at.is_none_or(|exp| now < exp))
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
        let indexed_version = current.indexed_version;
        let gseq = self.global_seq.fetch_add(1, Ordering::SeqCst) + 1;
        entry.next_seq = gseq;
        entry.items.insert(
            id,
            RecordEntry {
                item: new_item.clone(),
                expires_at,
                seq: gseq,
                indexed_version,
                tombstone: false,
            },
        );
        Ok(())
    }

    async fn tombstone(&self, ns: &ProjectId, id: MemoryId) -> Result<Option<MemoryItem>> {
        let now = Instant::now();
        let mut guard = self.records.write().unwrap();
        let Some(entry) = guard.get_mut(ns) else {
            return Ok(None);
        };
        let Some(rec) = entry.items.get_mut(&id) else {
            return Ok(None);
        };
        if rec.tombstone || rec.expires_at.is_some_and(|exp| now >= exp) {
            return Ok(None);
        }
        let key_to_release = rec.item.idempotency_key.clone();
        rec.item.version += 1;
        rec.tombstone = true;
        let gseq = self.global_seq.fetch_add(1, Ordering::SeqCst) + 1;
        rec.seq = gseq;
        let result_item = rec.item.clone();
        entry.next_seq = gseq;
        if let Some(key) = key_to_release {
            if entry.create_keys.get(&key) == Some(&id) {
                entry.create_keys.remove(&key);
            }
        }
        Ok(Some(result_item))
    }

    async fn pending(&self, limit: usize) -> Result<Vec<PendingRecord>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let now = Instant::now();
        let guard = self.records.read().unwrap();
        let mut pending: Vec<(u64, PendingRecord)> = Vec::new();
        for ns_entry in guard.values() {
            for e in ns_entry.items.values() {
                if e.indexed_version != e.item.version && e.expires_at.is_none_or(|exp| now < exp) {
                    pending.push((
                        e.seq,
                        PendingRecord {
                            item: e.item.clone(),
                            tombstone: e.tombstone,
                        },
                    ));
                }
            }
        }
        pending.sort_by_key(|(seq, _)| *seq);
        Ok(pending.into_iter().take(limit).map(|(_, r)| r).collect())
    }

    async fn pending_for(&self, ns: &ProjectId, limit: usize) -> Result<Vec<MemoryItem>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let now = Instant::now();
        let guard = self.records.read().unwrap();
        let Some(entry) = guard.get(ns) else {
            return Ok(Vec::new());
        };
        let mut pending: Vec<(u64, MemoryItem)> = entry
            .items
            .values()
            .filter(|e| {
                !e.tombstone
                    && e.indexed_version != e.item.version
                    && e.expires_at.is_none_or(|exp| now < exp)
            })
            .map(|e| (e.seq, e.item.clone()))
            .collect();
        pending.sort_by_key(|(seq, _)| *seq);
        Ok(pending.into_iter().take(limit).map(|(_, i)| i).collect())
    }

    async fn mark_indexed(&self, ns: &ProjectId, id: MemoryId, version: u64) -> Result<()> {
        let mut guard = self.records.write().unwrap();
        let Some(entry) = guard.get_mut(ns) else {
            return Ok(());
        };
        if let Some(rec) = entry.items.get_mut(&id) {
            if rec.item.version == version {
                if rec.tombstone {
                    entry.items.remove(&id);
                } else {
                    rec.indexed_version = version;
                }
            }
        }
        Ok(())
    }

    async fn session_items(
        &self,
        ns: &ProjectId,
        session: &SessionId,
        limit: usize,
    ) -> Result<Vec<MemoryItem>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let now = Instant::now();
        let guard = self.records.read().unwrap();
        let Some(entry) = guard.get(ns) else {
            return Ok(Vec::new());
        };
        let mut items: Vec<&RecordEntry> = entry
            .items
            .values()
            .filter(|e| !e.tombstone && e.expires_at.is_none_or(|exp| now < exp))
            .filter(|e| matches!(&e.item.scope, MemoryScope::Session(s) if s == session))
            .collect();
        items.sort_by_key(|b| std::cmp::Reverse(b.seq));
        Ok(items
            .into_iter()
            .take(limit)
            .map(|e| e.item.clone())
            .collect())
    }

    async fn recent(
        &self,
        ns: &ProjectId,
        scope: Option<MemoryScope>,
        limit: usize,
    ) -> Result<Vec<MemoryItem>> {
        let now = Instant::now();
        let guard = self.records.read().unwrap();
        let Some(entry) = guard.get(ns) else {
            return Ok(Vec::new());
        };
        let mut items: Vec<&RecordEntry> = entry
            .items
            .values()
            .filter(|e| !e.tombstone && e.expires_at.is_none_or(|exp| now < exp))
            .filter(|e| scope.as_ref().is_none_or(|s| &e.item.scope == s))
            .collect();
        items.sort_by_key(|b| std::cmp::Reverse(b.seq));
        Ok(items
            .into_iter()
            .take(limit)
            .map(|e| e.item.clone())
            .collect())
    }

    async fn delete(&self, ns: &ProjectId, id: MemoryId) -> Result<()> {
        let mut guard = self.records.write().unwrap();
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

    async fn describe(&self, ns: &ProjectId) -> Result<Option<IndexSpec>> {
        let guard = self.vectors.read().unwrap();
        Ok(guard.get(ns).and_then(|entry| {
            entry.dimensions.map(|dimensions| IndexSpec {
                dimensions,
                metric: entry.metric,
            })
        }))
    }

    async fn upsert_batch(&self, ns: &ProjectId, records: &[VectorRecord]) -> Result<()> {
        self.maybe_inject_fault(FaultPoint::VectorUpsert)?;
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
        self.maybe_inject_fault(FaultPoint::VectorDelete)?;
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
                Some(expr) => expr.matches(&rec.metadata),
                None => true,
            })
            .map(|rec| VectorMatch {
                id: rec.id,
                score: entry.metric.score(query_vector, &rec.vector),
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
        self.maybe_inject_fault(FaultPoint::GraphUpsert)?;
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
        self.maybe_inject_fault(FaultPoint::GraphUpsert)?;
        let mut guard = self.graphs.write().unwrap();
        let entry = guard.entry(ns.clone()).or_default();
        for rel in relations {
            let src_idx = ensure_node(entry, &rel.source);
            let dst_idx = ensure_node(entry, &rel.target);
            let new_edge = GraphEdge {
                source: rel.source.clone(),
                target: rel.target.clone(),
                relation_type: rel.relation_type.clone(),
                source_memory_id,
                valid_at: rel.valid_at,
                invalid_at: rel.invalid_at,
            };
            let existing = entry
                .graph
                .edges_connecting(src_idx, dst_idx)
                .find(|edge_ref| {
                    let w = edge_ref.weight();
                    w.source_memory_id == source_memory_id
                        && w.source == rel.source
                        && w.target == rel.target
                        && w.relation_type == rel.relation_type
                })
                .map(|edge_ref| edge_ref.id());
            if let Some(edge_idx) = existing {
                entry.graph[edge_idx] = new_edge;
            } else {
                entry.graph.add_edge(src_idx, dst_idx, new_edge);
            }
        }
        Ok(())
    }

    async fn retract_memory_facts(
        &self,
        ns: &ProjectId,
        source_memory_id: MemoryId,
        invalidated_at: DateTime<Utc>,
    ) -> Result<()> {
        self.maybe_inject_fault(FaultPoint::GraphRetract)?;
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
        let not_invalid = edge.invalid_at.is_none_or(|inv| as_of < inv);
        if !(valid && not_invalid) {
            return false;
        }
    }
    !(temporal.active_only && edge.invalid_at.is_some())
}
