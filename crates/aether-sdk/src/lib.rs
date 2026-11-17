//! Public facade and composition root for Aether SDK.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use aether_adapters::{Batcher, Resilient, RetryPolicy, Timeout, Traced};
use aether_core::{
    AetherError, ConsistencyMode, CreateOutcome, DistanceMetric, EntityId, FilterExpr, GraphStore,
    MemoryId, MemoryInput, MemoryItem, MemoryReceipt, MemoryScope, MemoryUpdate, PendingRecord,
    ProjectId, RecallHit, RecallMode, RecallQuery, RecallResult, ReceiptState, RecordStore, Result,
    Subgraph, VectorStore, WriteDurability,
};
use aether_cortex::recall::{self, Candidate, RRF_K};
use aether_cortex::{index_metadata, Indexer, PersonalizedPageRank};
use chrono::Utc;
use tokio::task::JoinHandle;
use tracing::Instrument;

pub use aether_adapters as adapters;
pub use aether_core as core;
pub use aether_cortex as cortex;

#[cfg(feature = "otel")]
pub mod telemetry;

const DEFAULT_SESSION_TTL: Duration = Duration::from_secs(3600);
const DEFAULT_CONNECTOR_TIMEOUT: Duration = Duration::from_secs(10);

/// Builder for [`MemoryManager`] assembling `RecordStore` and optional vector/graph stores.
pub struct MemoryManagerBuilder {
    record_store: Arc<dyn RecordStore>,
    vector_store: Option<Arc<dyn VectorStore>>,
    graph_store: Option<Arc<dyn GraphStore>>,
    background_indexing: Option<Duration>,
    connector_tracing: bool,
    session_ttl: Option<Duration>,
    timeout: Option<Duration>,
    retry: Option<RetryPolicy>,
}

impl MemoryManagerBuilder {
    /// Attaches an optional derived [`VectorStore`] index.
    pub fn vector(mut self, vector_store: Arc<dyn VectorStore>) -> Self {
        self.vector_store = Some(vector_store);
        self
    }

    /// Attaches an optional derived [`GraphStore`] index.
    pub fn graph(mut self, graph_store: Arc<dyn GraphStore>) -> Self {
        self.graph_store = Some(graph_store);
        self
    }

    /// Enables an opt-in background [`Indexer`] loop polling at `interval` (disabled by default).
    pub fn background_indexing(mut self, interval: Option<Duration>) -> Self {
        self.background_indexing = interval;
        self
    }

    /// Enables or disables automatic [`Traced`] connector span wrapping (enabled by default).
    pub fn with_connector_tracing(mut self, enabled: bool) -> Self {
        self.connector_tracing = enabled;
        self
    }

    /// Sets the TTL applied to `MemoryScope::Session` entries (default: 1 hour).
    pub fn with_session_ttl(mut self, ttl: Option<Duration>) -> Self {
        self.session_ttl = ttl;
        self
    }

    /// Sets the per-connector timeout wrapper (default: 10 seconds).
    pub fn with_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.timeout = timeout;
        self
    }

    /// Sets the transient retry policy wrapper (default: [`RetryPolicy::default`]).
    pub fn with_retry(mut self, policy: Option<RetryPolicy>) -> Self {
        self.retry = policy;
        self
    }

    /// Builds a [`MemoryManager`] with middleware stacked innermost-first:
    /// `Batcher` (vector) -> `Timeout` -> `Resilient` -> `Traced`.
    pub fn build(self) -> MemoryManager {
        let record_store = wrap_record(
            self.record_store,
            self.timeout,
            self.retry,
            self.connector_tracing,
        );
        let vector_store = self
            .vector_store
            .map(|vs| wrap_vector(vs, self.timeout, self.retry, self.connector_tracing));
        let graph_store = self
            .graph_store
            .map(|gs| wrap_graph(gs, self.timeout, self.retry, self.connector_tracing));

        let mut indexer = Indexer::new(Arc::clone(&record_store));
        if let Some(vs) = &vector_store {
            indexer = indexer.with_vector(Arc::clone(vs));
        }
        if let Some(gs) = &graph_store {
            indexer = indexer.with_graph(Arc::clone(gs));
        }
        let indexer = Arc::new(indexer);
        let bg_task = self
            .background_indexing
            .map(|interval| indexer.spawn(interval));

        MemoryManager {
            record_store,
            vector_store,
            graph_store,
            indexer,
            ppr: PersonalizedPageRank::default(),
            session_ttl: self.session_ttl,
            bg_task: Mutex::new(bg_task),
        }
    }
}

fn wrap_record(
    store: Arc<dyn RecordStore>,
    timeout: Option<Duration>,
    retry: Option<RetryPolicy>,
    traced: bool,
) -> Arc<dyn RecordStore> {
    let store: Arc<dyn RecordStore> = match timeout {
        Some(d) => Arc::new(Timeout::new(store, d)),
        None => store,
    };
    let store: Arc<dyn RecordStore> = match retry {
        Some(p) => Arc::new(Resilient::new(store, p)),
        None => store,
    };
    if traced {
        Arc::new(Traced::new(store))
    } else {
        store
    }
}

fn wrap_vector(
    store: Arc<dyn VectorStore>,
    timeout: Option<Duration>,
    retry: Option<RetryPolicy>,
    traced: bool,
) -> Arc<dyn VectorStore> {
    let store: Arc<dyn VectorStore> = Arc::new(Batcher::new(store));
    let store: Arc<dyn VectorStore> = match timeout {
        Some(d) => Arc::new(Timeout::new(store, d)),
        None => store,
    };
    let store: Arc<dyn VectorStore> = match retry {
        Some(p) => Arc::new(Resilient::new(store, p)),
        None => store,
    };
    if traced {
        Arc::new(Traced::new(store))
    } else {
        store
    }
}

fn wrap_graph(
    store: Arc<dyn GraphStore>,
    timeout: Option<Duration>,
    retry: Option<RetryPolicy>,
    traced: bool,
) -> Arc<dyn GraphStore> {
    let store: Arc<dyn GraphStore> = match timeout {
        Some(d) => Arc::new(Timeout::new(store, d)),
        None => store,
    };
    let store: Arc<dyn GraphStore> = match retry {
        Some(p) => Arc::new(Resilient::new(store, p)),
        None => store,
    };
    if traced {
        Arc::new(Traced::new(store))
    } else {
        store
    }
}

/// Primary non-generic SDK facade orchestrating record, vector, and graph stores.
pub struct MemoryManager {
    record_store: Arc<dyn RecordStore>,
    vector_store: Option<Arc<dyn VectorStore>>,
    graph_store: Option<Arc<dyn GraphStore>>,
    indexer: Arc<Indexer>,
    ppr: PersonalizedPageRank,
    session_ttl: Option<Duration>,
    bg_task: Mutex<Option<JoinHandle<()>>>,
}

impl Drop for MemoryManager {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.bg_task.lock() {
            if let Some(handle) = guard.take() {
                handle.abort();
            }
        }
    }
}

impl MemoryManager {
    /// Starts configuring a [`MemoryManager`] backed by `record_store`.
    pub fn builder(record_store: Arc<dyn RecordStore>) -> MemoryManagerBuilder {
        MemoryManagerBuilder {
            record_store,
            vector_store: None,
            graph_store: None,
            background_indexing: None,
            connector_tracing: true,
            session_ttl: Some(DEFAULT_SESSION_TTL),
            timeout: Some(DEFAULT_CONNECTOR_TIMEOUT),
            retry: Some(RetryPolicy::default()),
        }
    }

    fn ttl_for(&self, scope: &MemoryScope) -> Option<Duration> {
        match scope {
            MemoryScope::Session(_) => self.session_ttl,
            MemoryScope::Project | MemoryScope::User(_) => None,
        }
    }

    async fn check_vector_spec(&self, ns: &ProjectId, vector: &[f32]) -> Result<()> {
        let Some(vs) = &self.vector_store else {
            return Ok(());
        };
        let spec = vs.describe(ns).await?.ok_or_else(|| {
            AetherError::InvalidInput(format!("project `{ns}` vector index is not initialized"))
        })?;
        spec.check(vector)
    }

    /// Persists a new memory in the authoritative record store and optionally syncs derived indexes.
    pub async fn remember(&self, input: MemoryInput) -> Result<MemoryReceipt> {
        let span = tracing::info_span!(
            "aether.remember",
            project_id = %input.project_id,
            scope = ?input.scope,
            durability = ?input.durability,
            otel.status_code = tracing::field::Empty,
        );
        run_spanned(span, async {
            input.validate()?;
            if let Some(vec) = &input.embedding {
                if !matches!(input.scope, MemoryScope::Session(_)) {
                    self.check_vector_spec(&input.project_id, vec).await?;
                }
            }

            let durability = input.durability;
            let item = MemoryItem::from_input(input, 1);
            let ttl = self.ttl_for(&item.scope);
            match self
                .record_store
                .create(&item.project_id, &item, ttl)
                .await?
            {
                CreateOutcome::Duplicate(receipt) => Ok(receipt),
                CreateOutcome::Created => match durability {
                    WriteDurability::WaitFlush => {
                        let pending = PendingRecord {
                            item: item.clone(),
                            tombstone: false,
                        };
                        self.indexer.apply(&pending).await?;
                        Ok(MemoryReceipt {
                            memory_id: item.id,
                            state: ReceiptState::Persisted,
                            version: item.version,
                        })
                    }
                    WriteDurability::Buffered => {
                        self.indexer.notify();
                        Ok(MemoryReceipt {
                            memory_id: item.id,
                            state: ReceiptState::Buffered,
                            version: item.version,
                        })
                    }
                },
            }
        })
        .await
    }

    /// Queries memory using vector, graph, or hybrid recall under the requested consistency mode.
    pub async fn recall(&self, query: RecallQuery) -> Result<RecallResult> {
        let span = tracing::info_span!(
            "aether.recall",
            project_id = %query.project_id,
            mode = ?query.mode,
            consistency = ?query.consistency,
            top_k = query.top_k,
            otel.status_code = tracing::field::Empty,
        );
        run_spanned(span, async {
            query.validate()?;
            self.validate_store_availability(&query)?;

            if query.top_k == 0 {
                return Ok(RecallResult::default());
            }

            match query.mode {
                RecallMode::Vector => {
                    let candidates = self.vector_recall_branch(&query).await?;
                    Ok(to_result(candidates, Subgraph::default(), query.top_k))
                }
                RecallMode::Graph => {
                    let gs = self.graph_store.as_ref().ok_or_else(|| {
                        AetherError::InvalidInput("graph store is not configured".into())
                    })?;
                    let (cands, subgraph) = recall::graph_search(
                        &**gs,
                        &*self.record_store,
                        &query.project_id,
                        &query.seed_entities,
                        query.hops,
                        query.per_node_limit,
                        &query.temporal,
                        &self.ppr,
                    )
                    .await?;
                    let filtered = filter_graph_candidates(cands, &query);
                    Ok(to_result(filtered, subgraph, query.top_k))
                }
                RecallMode::Hybrid => {
                    let vec_candidates = self.vector_recall_branch(&query).await?;
                    let seeds = resolve_hybrid_seeds(&query.seed_entities, &vec_candidates);
                    if let (Some(gs), false) = (&self.graph_store, seeds.is_empty()) {
                        let (graph_raw, subgraph) = recall::graph_search(
                            &**gs,
                            &*self.record_store,
                            &query.project_id,
                            &seeds,
                            query.hops,
                            query.per_node_limit,
                            &query.temporal,
                            &self.ppr,
                        )
                        .await?;
                        let graph_cands = filter_graph_candidates(graph_raw, &query);
                        let fused = recall::rrf(&[vec_candidates, graph_cands], RRF_K);
                        Ok(to_result(fused, subgraph, query.top_k))
                    } else {
                        Ok(to_result(vec_candidates, Subgraph::default(), query.top_k))
                    }
                }
            }
        })
        .await
    }

    fn validate_store_availability(&self, query: &RecallQuery) -> Result<()> {
        match query.mode {
            RecallMode::Vector | RecallMode::Hybrid if self.vector_store.is_none() => Err(
                AetherError::InvalidInput("vector store is not configured".into()),
            ),
            RecallMode::Graph if self.graph_store.is_none() => Err(AetherError::InvalidInput(
                "graph store is not configured".into(),
            )),
            _ => Ok(()),
        }
    }

    async fn vector_recall_branch(&self, query: &RecallQuery) -> Result<Vec<Candidate>> {
        let vs = self
            .vector_store
            .as_ref()
            .ok_or_else(|| AetherError::InvalidInput("vector store is not configured".into()))?;
        let qv = query.query_vector.as_deref().ok_or_else(|| {
            AetherError::InvalidInput("query_vector is required for vector or hybrid recall".into())
        })?;

        let metric = match vs.describe(&query.project_id).await? {
            Some(spec) => {
                spec.check(qv)?;
                spec.metric
            }
            None => DistanceMetric::Cosine,
        };

        let mut lists = Vec::new();
        let overfetch = query.top_k.saturating_mul(2).max(1);

        if let Some(scope_expr) = recall::scope_filter(&query.scopes) {
            let combined = match &query.filters {
                Some(extra) => FilterExpr::And(vec![scope_expr, extra.clone()]),
                None => scope_expr,
            };
            let indexed = recall::vector_search(
                &**vs,
                &*self.record_store,
                &query.project_id,
                qv,
                overfetch,
                Some(&combined),
            )
            .await?;
            lists.push(recall::in_scopes(indexed, &query.scopes));
        }

        if query.consistency == ConsistencyMode::ReadYourWrites {
            let unindexed = recall::unindexed(
                &*self.record_store,
                &query.project_id,
                qv,
                metric,
                query.filters.as_ref(),
            )
            .await?;
            lists.push(recall::in_scopes(unindexed, &query.scopes));
        }

        let mut seen_sessions = HashSet::new();
        for scope in &query.scopes {
            if let MemoryScope::Session(sid) = scope {
                if seen_sessions.insert(sid.as_str().to_owned()) {
                    let sess = recall::session(
                        &*self.record_store,
                        &query.project_id,
                        sid,
                        qv,
                        metric,
                        query.filters.as_ref(),
                    )
                    .await?;
                    lists.push(sess);
                }
            }
        }

        Ok(recall::merge(lists))
    }

    /// Updates a live memory record via CAS and synchronously updates derived indexes.
    pub async fn update(
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
            patch.validate()?;
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

            if current.embedding.is_some() && patch.content.is_some() && patch.embedding.is_none() {
                return Err(AetherError::InvalidInput(
                    "updating content of an embedded memory requires a new embedding".into(),
                ));
            }
            if let Some(vec) = &patch.embedding {
                if !matches!(current.scope, MemoryScope::Session(_)) {
                    self.check_vector_spec(project_id, vec).await?;
                }
            }

            if let Some(content) = patch.content {
                current.content = content;
            }
            if let Some(embedding) = patch.embedding {
                current.embedding = Some(embedding);
            }
            if let Some(metadata) = patch.metadata {
                current.metadata = metadata;
            }
            if let Some(entities) = patch.entities {
                current.entities = entities;
            }
            if let Some(relations) = patch.relations {
                current.relations = relations;
            }
            current.version += 1;
            current.updated_at = Utc::now();

            self.record_store
                .cas_update(project_id, memory_id, patch.expected_version, &current)
                .await?;
            let pending = PendingRecord {
                item: current.clone(),
                tombstone: false,
            };
            self.indexer.apply(&pending).await?;
            Ok(MemoryReceipt {
                memory_id,
                state: ReceiptState::Persisted,
                version: current.version,
            })
        })
        .await
    }

    /// Tombstones a memory immediately so it disappears from reads, then retracts derived indexes.
    pub async fn forget(&self, project_id: &ProjectId, memory_id: MemoryId) -> Result<()> {
        let span = tracing::info_span!(
            "aether.forget",
            project_id = %project_id,
            memory_id = %memory_id,
            otel.status_code = tracing::field::Empty,
        );
        run_spanned(span, async {
            if let Some(item) = self.record_store.tombstone(project_id, memory_id).await? {
                let pending = PendingRecord {
                    item,
                    tombstone: true,
                };
                self.indexer.apply(&pending).await?;
            }
            Ok(())
        })
        .await
    }

    /// Drains all pending unindexed records and tombstones through the [`Indexer`].
    pub async fn flush(&self) -> Result<()> {
        let span = tracing::info_span!("aether.flush", otel.status_code = tracing::field::Empty);
        run_spanned(span, async {
            self.indexer.run_pass().await?;
            Ok(())
        })
        .await
    }

    /// Stops any background indexer loop, flushes pending records, and closes stores.
    pub async fn shutdown(&self) -> Result<()> {
        let span = tracing::info_span!("aether.shutdown", otel.status_code = tracing::field::Empty);
        run_spanned(span, async {
            let handle = self.bg_task.lock().ok().and_then(|mut g| g.take());
            if let Some(h) = handle {
                h.abort();
                let _ = h.await;
            }
            self.indexer.run_pass().await?;
            self.record_store.close().await?;
            if let Some(vs) = &self.vector_store {
                vs.close().await?;
            }
            if let Some(gs) = &self.graph_store {
                gs.close().await?;
            }
            Ok(())
        })
        .await
    }
}

fn filter_graph_candidates(candidates: Vec<Candidate>, query: &RecallQuery) -> Vec<Candidate> {
    let scoped = recall::in_scopes(candidates, &query.scopes);
    match &query.filters {
        Some(expr) => scoped
            .into_iter()
            .filter(|c| expr.matches(&index_metadata(&c.item)))
            .collect(),
        None => scoped,
    }
}

fn resolve_hybrid_seeds(explicit: &[EntityId], vec_candidates: &[Candidate]) -> Vec<EntityId> {
    if !explicit.is_empty() {
        return explicit.to_vec();
    }
    let mut seen = HashSet::new();
    let mut seeds = Vec::new();
    for cand in vec_candidates {
        for entity in &cand.item.entities {
            if seen.insert(entity.id.as_str().to_owned()) {
                seeds.push(entity.id.clone());
            }
        }
    }
    seeds
}

fn to_result(mut candidates: Vec<Candidate>, subgraph: Subgraph, top_k: usize) -> RecallResult {
    candidates.truncate(top_k);
    let hits = candidates
        .into_iter()
        .map(|c| RecallHit {
            memory_id: c.item.id,
            content: c.item.content,
            score: c.score,
            metadata: c.item.metadata,
        })
        .collect();
    RecallResult { hits, subgraph }
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
