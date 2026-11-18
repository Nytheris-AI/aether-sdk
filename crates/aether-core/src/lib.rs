//! Core domain contracts, DTOs, errors, and storage traits for Aether SDK.
//!
//! ```
//! use nytheris_aether_core::{DistanceMetric, FilterExpr, FilterValue, ProjectId};
//! use serde_json::json;
//!
//! let project = ProjectId::new("demo-project")?;
//! assert_eq!(project.as_str(), "demo-project");
//!
//! let score = DistanceMetric::Cosine.score(&[1.0, 0.0], &[1.0, 0.0]);
//! assert!((score - 1.0).abs() < 1e-6);
//!
//! let filter = FilterExpr::Eq {
//!     field: "topic".into(),
//!     value: FilterValue::String("rust".into()),
//! };
//! assert!(filter.matches(&json!({"topic": "rust"})));
//! # Ok::<(), nytheris_aether_core::AetherError>(())
//! ```

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use uuid::Uuid;

/// Unified error type across Aether SDK and outbound storage connectors.
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
            return Err(AetherError::InvalidInput(
                "project_id must not be empty".into(),
            ));
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
            return Err(AetherError::InvalidInput(
                "entity_id must not be empty".into(),
            ));
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

/// Strongly typed user identifier for user-scoped memories.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct UserId(String);

impl UserId {
    /// Validates and constructs a non-empty `UserId`.
    pub fn new(id: impl Into<String>) -> Result<Self> {
        let s = id.into();
        if s.trim().is_empty() {
            return Err(AetherError::InvalidInput(
                "user_id must not be empty".into(),
            ));
        }
        Ok(Self(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for UserId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Strongly typed session identifier for working-memory entries.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(String);

impl SessionId {
    /// Validates and constructs a non-empty `SessionId`.
    pub fn new(id: impl Into<String>) -> Result<Self> {
        let s = id.into();
        if s.trim().is_empty() {
            return Err(AetherError::InvalidInput(
                "session_id must not be empty".into(),
            ));
        }
        Ok(Self(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Opaque credential wrapper whose `Debug` and `Display` implementations always redact the value.
#[derive(Clone, PartialEq, Eq)]
pub struct SecretString(String);

impl SecretString {
    pub fn new(secret: impl Into<String>) -> Self {
        Self(secret.into())
    }

    pub fn expose_secret(&self) -> &str {
        &self.0
    }
}

impl From<String> for SecretString {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for SecretString {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

impl fmt::Display for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

/// Strips `userinfo@` credentials from the authority portion of a URL or DSN string.
pub fn redact_url_userinfo(raw: &str) -> String {
    let Some((scheme, rest)) = raw.split_once("://") else {
        return raw.to_string();
    };
    let auth_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, suffix) = rest.split_at(auth_end);
    let Some((_, host)) = authority.rsplit_once('@') else {
        return raw.to_string();
    };
    format!("{scheme}://[REDACTED]@{host}{suffix}")
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

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum MemoryScope {
    #[default]
    Project,
    User(UserId),
    Session(SessionId),
}

impl MemoryScope {
    /// Returns the index metadata key (`"project"` or `"user:<id>"`).
    ///
    /// Session memories are never written to derived vector/graph indexes, so this returns `None`.
    pub fn index_key(&self) -> Option<String> {
        match self {
            MemoryScope::Project => Some("project".into()),
            MemoryScope::User(uid) => Some(format!("user:{}", uid.as_str())),
            MemoryScope::Session(_) => None,
        }
    }
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

impl DistanceMetric {
    /// Computes similarity score between two vectors such that higher values indicate closer vectors.
    ///
    /// For Cosine, zero-norm vectors yield 0.0.
    /// For Euclidean, the negative Euclidean distance is returned so that closer vectors have a higher score.
    pub fn score(&self, a: &[f32], b: &[f32]) -> f32 {
        match self {
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

impl FilterExpr {
    /// Evaluates whether the given JSON metadata matches this filter expression.
    pub fn matches(&self, metadata: &Value) -> bool {
        match self {
            FilterExpr::Eq { field, value } => {
                metadata.get(field).is_some_and(|v| value_matches(v, value))
            }
            FilterExpr::In { field, values } => metadata
                .get(field)
                .is_some_and(|v| values.iter().any(|target| value_matches(v, target))),
            FilterExpr::Range { field, gte, lte } => metadata
                .get(field)
                .and_then(|v| v.as_f64())
                .is_some_and(|num| {
                    gte.is_none_or(|min| num >= min) && lte.is_none_or(|max| num <= max)
                }),
            FilterExpr::And(exprs) => exprs.iter().all(|e| e.matches(metadata)),
            FilterExpr::Or(exprs) => exprs.iter().any(|e| e.matches(metadata)),
        }
    }
}

fn value_matches(json_val: &Value, filter_val: &FilterValue) -> bool {
    match (json_val, filter_val) {
        (Value::String(a), FilterValue::String(b)) => a == b,
        (Value::Number(a), FilterValue::Int(b)) => a.as_i64() == Some(*b),
        (Value::Number(a), FilterValue::Float(b)) => {
            a.as_f64().is_some_and(|x| (x - b).abs() < f64::EPSILON)
        }
        (Value::Bool(a), FilterValue::Bool(b)) => a == b,
        _ => false,
    }
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

/// Caller-constructed memory payload validated at the SDK boundary.
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

    /// Validates required invariants before entering the SDK.
    pub fn validate(&self) -> Result<()> {
        if self.content.trim().is_empty() {
            return Err(AetherError::InvalidInput(
                "memory content must not be empty".into(),
            ));
        }
        if let Some(vec) = &self.embedding {
            validate_vector_values(vec, "embedding vector")?;
        }
        validate_metadata_object(&self.metadata)?;
        validate_entities_and_relations(&self.entities, &self.relations)?;
        Ok(())
    }
}

fn validate_vector_values(vec: &[f32], label: &str) -> Result<()> {
    if vec.is_empty() {
        return Err(AetherError::InvalidInput(format!(
            "{label} must not be empty when provided"
        )));
    }
    if vec.iter().any(|x| !x.is_finite()) {
        return Err(AetherError::InvalidInput(format!(
            "{label} contains non-finite values"
        )));
    }
    Ok(())
}

fn validate_metadata_object(metadata: &Value) -> Result<()> {
    let Some(map) = metadata.as_object() else {
        return Err(AetherError::InvalidInput(
            "metadata must be a JSON object".into(),
        ));
    };
    if let Some(key) = map.keys().find(|k| k.starts_with("_aether_")) {
        return Err(AetherError::InvalidInput(format!(
            "metadata key `{key}` uses reserved `_aether_` prefix"
        )));
    }
    Ok(())
}

fn validate_entities_and_relations(entities: &[Entity], relations: &[Relation]) -> Result<()> {
    for entity in entities {
        if entity.name.trim().is_empty() {
            return Err(AetherError::InvalidInput(
                "entity name must not be empty".into(),
            ));
        }
    }
    for rel in relations {
        if rel.relation_type.trim().is_empty() {
            return Err(AetherError::InvalidInput(
                "relation_type must not be empty".into(),
            ));
        }
    }
    Ok(())
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CreateOutcome {
    Created,
    Duplicate(MemoryReceipt),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PendingRecord {
    pub item: MemoryItem,
    pub tombstone: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexSpec {
    pub dimensions: usize,
    pub metric: DistanceMetric,
}

impl IndexSpec {
    /// Validates that `vector` is non-empty, finite, and matches `self.dimensions`.
    pub fn check(&self, vector: &[f32]) -> Result<()> {
        validate_vector_values(vector, "vector")?;
        if vector.len() != self.dimensions {
            return Err(AetherError::InvalidInput(format!(
                "vector dimension mismatch: expected {}, got {}",
                self.dimensions,
                vector.len()
            )));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryUpdate {
    pub content: Option<String>,
    pub embedding: Option<Vec<f32>>,
    pub metadata: Option<Value>,
    pub entities: Option<Vec<Entity>>,
    pub relations: Option<Vec<Relation>>,
    pub expected_version: Option<u64>,
}

impl MemoryUpdate {
    /// Validates updated fields at the boundary.
    pub fn validate(&self) -> Result<()> {
        if let Some(content) = &self.content {
            if content.trim().is_empty() {
                return Err(AetherError::InvalidInput(
                    "updated content must not be empty".into(),
                ));
            }
        }
        if let Some(vec) = &self.embedding {
            validate_vector_values(vec, "embedding vector")?;
        }
        if let Some(metadata) = &self.metadata {
            validate_metadata_object(metadata)?;
        }
        validate_entities_and_relations(
            self.entities.as_deref().unwrap_or(&[]),
            self.relations.as_deref().unwrap_or(&[]),
        )?;
        Ok(())
    }
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
    pub scopes: Vec<MemoryScope>,
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
            scopes: vec![MemoryScope::Project],
            consistency: ConsistencyMode::default(),
        }
    }

    /// Validates query requirements before dispatching to stores.
    pub fn validate(&self) -> Result<()> {
        if self.scopes.is_empty() {
            return Err(AetherError::InvalidInput("scopes must not be empty".into()));
        }
        if let Some(vec) = &self.query_vector {
            validate_vector_values(vec, "query_vector")?;
        }
        match self.mode {
            RecallMode::Vector | RecallMode::Hybrid if self.query_vector.is_none() => {
                return Err(AetherError::InvalidInput(
                    "query_vector is required for vector or hybrid recall".into(),
                ));
            }
            RecallMode::Graph if self.seed_entities.is_empty() => {
                return Err(AetherError::InvalidInput(
                    "seed_entities must not be empty for graph recall".into(),
                ));
            }
            _ => {}
        }
        Ok(())
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

/// Unified record store contract (SQLite, Redis/Valkey, etc.).
#[async_trait]
pub trait RecordStore: OutboundConnector {
    async fn create(
        &self,
        ns: &ProjectId,
        item: &MemoryItem,
        ttl: Option<Duration>,
    ) -> Result<CreateOutcome>;

    async fn get(&self, ns: &ProjectId, id: MemoryId) -> Result<Option<MemoryItem>>;

    async fn get_many(&self, ns: &ProjectId, ids: &[MemoryId]) -> Result<Vec<MemoryItem>>;

    async fn cas_update(
        &self,
        ns: &ProjectId,
        id: MemoryId,
        expected_version: Option<u64>,
        new_item: &MemoryItem,
    ) -> Result<()>;

    async fn tombstone(&self, ns: &ProjectId, id: MemoryId) -> Result<Option<MemoryItem>>;

    async fn pending(&self, limit: usize) -> Result<Vec<PendingRecord>>;

    async fn pending_for(&self, ns: &ProjectId, limit: usize) -> Result<Vec<MemoryItem>>;

    async fn session_items(
        &self,
        ns: &ProjectId,
        session: &SessionId,
        limit: usize,
    ) -> Result<Vec<MemoryItem>>;

    async fn mark_indexed(&self, ns: &ProjectId, id: MemoryId, version: u64) -> Result<()>;
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

    async fn describe(&self, ns: &ProjectId) -> Result<Option<IndexSpec>>;

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

    /// Upserts provenance-tagged relations keyed by `(source_memory_id, source, target, relation_type)`.
    ///
    /// Repeating an upsert for an existing key replaces the stored edge in place (including
    /// restoring an active `invalid_at = None` edge after prior retraction) rather than adding duplicates.
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
impl<T: RecordStore + ?Sized> RecordStore for Arc<T> {
    async fn create(
        &self,
        ns: &ProjectId,
        item: &MemoryItem,
        ttl: Option<Duration>,
    ) -> Result<CreateOutcome> {
        (**self).create(ns, item, ttl).await
    }

    async fn get(&self, ns: &ProjectId, id: MemoryId) -> Result<Option<MemoryItem>> {
        (**self).get(ns, id).await
    }

    async fn get_many(&self, ns: &ProjectId, ids: &[MemoryId]) -> Result<Vec<MemoryItem>> {
        (**self).get_many(ns, ids).await
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

    async fn tombstone(&self, ns: &ProjectId, id: MemoryId) -> Result<Option<MemoryItem>> {
        (**self).tombstone(ns, id).await
    }

    async fn pending(&self, limit: usize) -> Result<Vec<PendingRecord>> {
        (**self).pending(limit).await
    }

    async fn pending_for(&self, ns: &ProjectId, limit: usize) -> Result<Vec<MemoryItem>> {
        (**self).pending_for(ns, limit).await
    }

    async fn session_items(
        &self,
        ns: &ProjectId,
        session: &SessionId,
        limit: usize,
    ) -> Result<Vec<MemoryItem>> {
        (**self).session_items(ns, session, limit).await
    }

    async fn mark_indexed(&self, ns: &ProjectId, id: MemoryId, version: u64) -> Result<()> {
        (**self).mark_indexed(ns, id, version).await
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

    async fn describe(&self, ns: &ProjectId) -> Result<Option<IndexSpec>> {
        (**self).describe(ns).await
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
