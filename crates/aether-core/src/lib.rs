//! Core domain contracts, DTOs, errors, and inbound/outbound traits for Aether SDK.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use uuid::Uuid;

/// Unified error type across the Aether gateway and outbound storage connectors.
#[derive(Debug, Error)]
pub enum AetherError {
    #[error("not implemented: {0}")]
    NotImplemented(&'static str),

    #[error("invalid input: {0}")]
    InvalidInput(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("version conflict on memory {memory_id}: expected {expected_version}, actual {actual_version}")]
    Conflict {
        memory_id: MemoryId,
        expected_version: u64,
        actual_version: u64,
    },

    #[error("transient error in connector `{connector}`: {message}")]
    Transient {
        connector: &'static str,
        message: String,
    },

    #[error("permanent error in connector `{connector}`: {message}")]
    Permanent {
        connector: &'static str,
        message: String,
    },

    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

impl AetherError {
    /// Returns `true` when the failure is transient and safe for bounded retries.
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Transient { .. })
    }
}

pub type Result<T> = std::result::Result<T, AetherError>;

/// Strongly typed tenant/project namespace enforced at every inbound and outbound boundary.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProjectId(String);

impl ProjectId {
    /// Validates and constructs a non-empty `ProjectId`.
    pub fn new(id: impl Into<String>) -> Result<Self> {
        let s = id.into();
        if s.trim().is_empty() {
            return Err(AetherError::InvalidInput("project_id must not be empty".into()));
        }
        Ok(Self(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Strongly typed memory identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MemoryId(pub Uuid);

impl MemoryId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn from_uuid(id: Uuid) -> Self {
        Self(id)
    }
}

impl Default for MemoryId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for MemoryId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Canonical entity identifier (e.g. `person:alice` or `db:turbopuffer`) to avoid surface-name collisions.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EntityId(String);

impl EntityId {
    /// Validates and constructs a non-empty `EntityId`.
    pub fn new(id: impl Into<String>) -> Result<Self> {
        let s = id.into();
        if s.trim().is_empty() {
            return Err(AetherError::InvalidInput("entity_id must not be empty".into()));
        }
        Ok(Self(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for EntityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MemoryTier {
    Episodic,
    #[default]
    Semantic,
    Procedural,
    Profile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MemoryScope {
    #[default]
    Project,
    User,
    Session,
    Global,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WriteDurability {
    #[default]
    Buffered,
    WaitFlush,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ConsistencyMode {
    Eventual,
    #[default]
    ReadYourWrites,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RecallMode {
    Vector,
    Graph,
    #[default]
    Hybrid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptState {
    Buffered,
    Persisted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DistanceMetric {
    #[default]
    Cosine,
    Dot,
    Euclidean,
}

/// Scalar value used inside portable `FilterExpr` predicates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FilterValue {
    String(String),
    Int(i64),
    Float(f64),
    Bool(bool),
}

/// Portable, backend-agnostic filter AST translated by each `VectorStore` driver.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", content = "args", rename_all = "snake_case")]
pub enum FilterExpr {
    Eq {
        field: String,
        value: FilterValue,
    },
    In {
        field: String,
        values: Vec<FilterValue>,
    },
    Range {
        field: String,
        gte: Option<f64>,
        lte: Option<f64>,
    },
    And(Vec<FilterExpr>),
    Or(Vec<FilterExpr>),
}

/// Point-in-time and active-edge filter for temporal `GraphStore` reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemporalFilter {
    pub as_of: Option<DateTime<Utc>>,
    pub active_only: bool,
}

impl Default for TemporalFilter {
    fn default() -> Self {
        Self {
            as_of: None,
            active_only: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entity {
    pub id: EntityId,
    pub name: String,
    pub entity_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Relation {
    pub source: EntityId,
    pub target: EntityId,
    pub relation_type: String,
    pub valid_at: DateTime<Utc>,
    pub invalid_at: Option<DateTime<Utc>>,
}

/// Caller-constructed inbound memory payload validated at the gateway boundary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryInput {
    pub project_id: ProjectId,
    pub content: String,
    pub embedding: Option<Vec<f32>>,
    pub entities: Vec<Entity>,
    pub relations: Vec<Relation>,
    pub tier: MemoryTier,
    pub scope: MemoryScope,
    pub durability: WriteDurability,
    pub idempotency_key: Option<String>,
    pub metadata: Value,
}

impl MemoryInput {
    pub fn new(project_id: ProjectId, content: impl Into<String>) -> Self {
        Self {
            project_id,
            content: content.into(),
            embedding: None,
            entities: Vec::new(),
            relations: Vec::new(),
            tier: MemoryTier::default(),
            scope: MemoryScope::default(),
            durability: WriteDurability::default(),
            idempotency_key: None,
            metadata: Value::Object(Default::default()),
        }
    }

    /// Validates required invariants before entering the gateway core.
    pub fn validate(&self) -> Result<()> {
        if self.content.trim().is_empty() {
            return Err(AetherError::InvalidInput(
                "memory content must not be empty".into(),
            ));
        }
        if let Some(vec) = &self.embedding {
            if vec.is_empty() {
                return Err(AetherError::InvalidInput(
                    "embedding vector must not be empty when provided".into(),
                ));
            }
            if vec.iter().any(|x| !x.is_finite()) {
                return Err(AetherError::InvalidInput(
                    "embedding vector contains non-finite values".into(),
                ));
            }
        }
        for entity in &self.entities {
            if entity.name.trim().is_empty() {
                return Err(AetherError::InvalidInput(
                    "entity name must not be empty".into(),
                ));
            }
        }
        for rel in &self.relations {
            if rel.relation_type.trim().is_empty() {
                return Err(AetherError::InvalidInput(
                    "relation_type must not be empty".into(),
                ));
            }
        }
        Ok(())
    }
}

/// Canonical stored memory record with optimistic-concurrency `version`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryItem {
    pub id: MemoryId,
    pub project_id: ProjectId,
    pub content: String,
    pub embedding: Option<Vec<f32>>,
    pub entities: Vec<Entity>,
    pub relations: Vec<Relation>,
    pub tier: MemoryTier,
    pub scope: MemoryScope,
    pub idempotency_key: Option<String>,
    pub metadata: Value,
    pub version: u64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl MemoryItem {
    pub fn from_input(input: MemoryInput, version: u64) -> Self {
        let now = Utc::now();
        Self {
            id: MemoryId::new(),
            project_id: input.project_id,
            content: input.content,
            embedding: input.embedding,
            entities: input.entities,
            relations: input.relations,
            tier: input.tier,
            scope: input.scope,
            idempotency_key: input.idempotency_key,
            metadata: input.metadata,
            version,
            created_at: now,
            updated_at: now,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryReceipt {
    pub memory_id: MemoryId,
    pub state: ReceiptState,
    pub version: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryUpdate {
    pub content: Option<String>,
    pub embedding: Option<Vec<f32>>,
    pub metadata: Option<Value>,
    pub expected_version: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecallQuery {
    pub project_id: ProjectId,
    pub query: String,
    pub query_vector: Option<Vec<f32>>,
    pub seed_entities: Vec<EntityId>,
    pub mode: RecallMode,
    pub top_k: usize,
    pub hops: u8,
    pub per_node_limit: usize,
    pub temporal: TemporalFilter,
    pub filters: Option<FilterExpr>,
    pub consistency: ConsistencyMode,
}

impl RecallQuery {
    pub fn new(project_id: ProjectId, query: impl Into<String>) -> Self {
        Self {
            project_id,
            query: query.into(),
            query_vector: None,
            seed_entities: Vec::new(),
            mode: RecallMode::default(),
            top_k: 10,
            hops: 2,
            per_node_limit: 25,
            temporal: TemporalFilter::default(),
            filters: None,
            consistency: ConsistencyMode::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecallHit {
    pub memory_id: MemoryId,
    pub content: String,
    pub score: f32,
    pub metadata: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct RecallResult {
    pub hits: Vec<RecallHit>,
    pub subgraph: Subgraph,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VectorRecord {
    pub id: MemoryId,
    pub vector: Vec<f32>,
    pub metadata: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VectorMatch {
    pub id: MemoryId,
    pub score: f32,
    pub metadata: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphEdge {
    pub source: EntityId,
    pub target: EntityId,
    pub relation_type: String,
    pub source_memory_id: MemoryId,
    pub valid_at: DateTime<Utc>,
    pub invalid_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Subgraph {
    pub entities: Vec<Entity>,
    pub edges: Vec<GraphEdge>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectorCapabilities {
    pub max_batch_size: usize,
    pub supports_temporal: bool,
    pub supports_filter_expr: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConnectorHealth {
    Healthy,
    Degraded(String),
    Unhealthy(String),
}

/// Base supertrait inherited by all outbound storage and database connectors.
#[async_trait]
pub trait OutboundConnector: Send + Sync {
    fn name(&self) -> &'static str;
    fn capabilities(&self) -> ConnectorCapabilities;
    async fn health_check(&self) -> Result<ConnectorHealth>;
    async fn close(&self) -> Result<()>;
}

/// Unified L2 working-memory / key-value store contract (Redis/Valkey, SQLite, etc.).
#[async_trait]
pub trait KvStore: OutboundConnector {
    async fn get(&self, ns: &ProjectId, id: MemoryId) -> Result<Option<MemoryItem>>;

    async fn put(&self, ns: &ProjectId, item: &MemoryItem, ttl: Option<Duration>) -> Result<()>;

    async fn put_if_absent(
        &self,
        ns: &ProjectId,
        idempotency_key: &str,
        receipt: &MemoryReceipt,
        ttl: Option<Duration>,
    ) -> Result<Option<MemoryReceipt>>;

    async fn cas_update(
        &self,
        ns: &ProjectId,
        id: MemoryId,
        expected_version: Option<u64>,
        new_item: &MemoryItem,
    ) -> Result<()>;

    async fn recent(
        &self,
        ns: &ProjectId,
        scope: Option<MemoryScope>,
        limit: usize,
    ) -> Result<Vec<MemoryItem>>;

    async fn delete(&self, ns: &ProjectId, id: MemoryId) -> Result<()>;
}

/// Unified vector storage contract (Turbopuffer, Qdrant, Chroma, pgvector, etc.).
#[async_trait]
pub trait VectorStore: OutboundConnector {
    async fn initialize(
        &self,
        ns: &ProjectId,
        dimensions: usize,
        metric: DistanceMetric,
    ) -> Result<()>;

    async fn upsert_batch(&self, ns: &ProjectId, records: &[VectorRecord]) -> Result<()>;

    async fn delete(&self, ns: &ProjectId, ids: &[MemoryId]) -> Result<()>;

    async fn search(
        &self,
        ns: &ProjectId,
        query_vector: &[f32],
        top_k: usize,
        filter: Option<&FilterExpr>,
    ) -> Result<Vec<VectorMatch>>;
}

/// Unified knowledge-graph storage contract (Neo4j, etc.) with provenance & temporal filtering.
#[async_trait]
pub trait GraphStore: OutboundConnector {
    async fn upsert_entities(&self, ns: &ProjectId, entities: &[Entity]) -> Result<()>;

    async fn upsert_relations(
        &self,
        ns: &ProjectId,
        source_memory_id: MemoryId,
        relations: &[Relation],
    ) -> Result<()>;

    async fn retract_memory_facts(
        &self,
        ns: &ProjectId,
        source_memory_id: MemoryId,
        invalidated_at: DateTime<Utc>,
    ) -> Result<()>;

    async fn neighborhood(
        &self,
        ns: &ProjectId,
        seeds: &[EntityId],
        hops: u8,
        per_node_limit: usize,
        temporal: &TemporalFilter,
    ) -> Result<Subgraph>;
}

/// Primary inbound gateway contract implemented by `MemoryManager`.
#[async_trait]
pub trait MemoryInbound: Send + Sync {
    async fn remember(&self, input: MemoryInput) -> Result<MemoryReceipt>;
    async fn recall(&self, query: RecallQuery) -> Result<RecallResult>;
    async fn update(
        &self,
        project_id: &ProjectId,
        memory_id: MemoryId,
        patch: MemoryUpdate,
    ) -> Result<MemoryReceipt>;
    async fn forget(&self, project_id: &ProjectId, memory_id: MemoryId) -> Result<()>;
    async fn flush(&self) -> Result<()>;
    async fn shutdown(&self) -> Result<()>;
}

#[async_trait]
impl<T: OutboundConnector + ?Sized> OutboundConnector for Arc<T> {
    fn name(&self) -> &'static str {
        (**self).name()
    }

    fn capabilities(&self) -> ConnectorCapabilities {
        (**self).capabilities()
    }

    async fn health_check(&self) -> Result<ConnectorHealth> {
        (**self).health_check().await
    }

    async fn close(&self) -> Result<()> {
        (**self).close().await
    }
}

#[async_trait]
impl<T: KvStore + ?Sized> KvStore for Arc<T> {
    async fn get(&self, ns: &ProjectId, id: MemoryId) -> Result<Option<MemoryItem>> {
        (**self).get(ns, id).await
    }

    async fn put(&self, ns: &ProjectId, item: &MemoryItem, ttl: Option<Duration>) -> Result<()> {
        (**self).put(ns, item, ttl).await
    }

    async fn put_if_absent(
        &self,
        ns: &ProjectId,
        idempotency_key: &str,
        receipt: &MemoryReceipt,
        ttl: Option<Duration>,
    ) -> Result<Option<MemoryReceipt>> {
        (**self)
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
        (**self)
            .cas_update(ns, id, expected_version, new_item)
            .await
    }

    async fn recent(
        &self,
        ns: &ProjectId,
        scope: Option<MemoryScope>,
        limit: usize,
    ) -> Result<Vec<MemoryItem>> {
        (**self).recent(ns, scope, limit).await
    }

    async fn delete(&self, ns: &ProjectId, id: MemoryId) -> Result<()> {
        (**self).delete(ns, id).await
    }
}

#[async_trait]
impl<T: VectorStore + ?Sized> VectorStore for Arc<T> {
    async fn initialize(
        &self,
        ns: &ProjectId,
        dimensions: usize,
        metric: DistanceMetric,
    ) -> Result<()> {
        (**self).initialize(ns, dimensions, metric).await
    }

    async fn upsert_batch(&self, ns: &ProjectId, records: &[VectorRecord]) -> Result<()> {
        (**self).upsert_batch(ns, records).await
    }

    async fn delete(&self, ns: &ProjectId, ids: &[MemoryId]) -> Result<()> {
        (**self).delete(ns, ids).await
    }

    async fn search(
        &self,
        ns: &ProjectId,
        query_vector: &[f32],
        top_k: usize,
        filter: Option<&FilterExpr>,
    ) -> Result<Vec<VectorMatch>> {
        (**self).search(ns, query_vector, top_k, filter).await
    }
}

#[async_trait]
impl<T: GraphStore + ?Sized> GraphStore for Arc<T> {
    async fn upsert_entities(&self, ns: &ProjectId, entities: &[Entity]) -> Result<()> {
        (**self).upsert_entities(ns, entities).await
    }

    async fn upsert_relations(
        &self,
        ns: &ProjectId,
        source_memory_id: MemoryId,
        relations: &[Relation],
    ) -> Result<()> {
        (**self)
            .upsert_relations(ns, source_memory_id, relations)
            .await
    }

    async fn retract_memory_facts(
        &self,
        ns: &ProjectId,
        source_memory_id: MemoryId,
        invalidated_at: DateTime<Utc>,
    ) -> Result<()> {
        (**self)
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
        (**self)
            .neighborhood(ns, seeds, hops, per_node_limit, temporal)
            .await
    }
}
