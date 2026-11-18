//! Chroma `VectorStore` connector.

use std::collections::HashMap;
use std::sync::RwLock;

use aether_core::{
    AetherError, ConnectorCapabilities, ConnectorHealth, DistanceMetric, FilterExpr, FilterValue,
    IndexSpec, MemoryId, OutboundConnector, ProjectId, Result, SecretString, VectorMatch,
    VectorRecord, VectorStore,
};
use async_trait::async_trait;
use reqwest::Client;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use super::http;

const CONNECTOR: &str = "chroma";
const DEFAULT_MAX_BATCH_SIZE: usize = 1000;

/// Configuration for connecting to a Chroma v2 REST API server.
#[derive(Debug, Clone)]
pub struct ChromaConfig {
    pub base_url: String,
    pub tenant: String,
    pub database: String,
    pub auth_token: Option<SecretString>,
    pub collection_prefix: String,
}

impl Default for ChromaConfig {
    fn default() -> Self {
        Self {
            base_url: "http://localhost:8000".to_string(),
            tenant: "default_tenant".to_string(),
            database: "default_database".to_string(),
            auth_token: None,
            collection_prefix: "aether".to_string(),
        }
    }
}

#[derive(Debug, Clone)]
struct CollectionSpec {
    id: String,
    dimensions: usize,
    metric: DistanceMetric,
}

/// Chroma v2 REST connector implementing `OutboundConnector` and `VectorStore`.
#[derive(Debug)]
pub struct ChromaVectorStore {
    client: Client,
    base_url: String,
    tenant: String,
    database: String,
    auth_token: Option<SecretString>,
    collection_prefix: String,
    collections: RwLock<HashMap<ProjectId, CollectionSpec>>,
}

impl ChromaVectorStore {
    /// Validates configuration and constructs a new `ChromaVectorStore`.
    pub fn new(config: ChromaConfig) -> Result<Self> {
        let base_url = http::normalize_base_url(&config.base_url)?;
        let tenant = config.tenant.trim().to_string();
        if tenant.is_empty() {
            return Err(AetherError::InvalidInput(
                "tenant must not be empty".to_string(),
            ));
        }
        let database = config.database.trim().to_string();
        if database.is_empty() {
            return Err(AetherError::InvalidInput(
                "database must not be empty".to_string(),
            ));
        }
        let client = Client::builder()
            .build()
            .map_err(|e| http::map_transport_err(CONNECTOR, e))?;

        Ok(Self {
            client,
            base_url,
            tenant,
            database,
            auth_token: config.auth_token,
            collection_prefix: config.collection_prefix.trim().to_string(),
            collections: RwLock::new(HashMap::new()),
        })
    }

    fn collection_name(&self, ns: &ProjectId) -> String {
        if self.collection_prefix.is_empty() {
            ns.as_str().to_string()
        } else {
            format!("{}_{}", self.collection_prefix, ns.as_str())
        }
    }

    fn collections_url(&self) -> String {
        format!(
            "{}/api/v2/tenants/{}/databases/{}/collections",
            self.base_url, self.tenant, self.database
        )
    }

    fn collection_by_name_url(&self, name: &str) -> String {
        format!(
            "{}/api/v2/tenants/{}/databases/{}/collections/{name}",
            self.base_url, self.tenant, self.database
        )
    }

    fn collection_op_url(&self, collection_id: &str, op: &str) -> String {
        format!(
            "{}/api/v2/tenants/{}/databases/{}/collections/{collection_id}/{op}",
            self.base_url, self.tenant, self.database
        )
    }

    fn apply_auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.auth_token {
            Some(token) => req.bearer_auth(token.expose_secret()),
            None => req,
        }
    }

    async fn post_json<B: Serialize + ?Sized>(
        &self,
        url: &str,
        body: &B,
    ) -> Result<reqwest::Response> {
        let req = self.apply_auth(self.client.post(url)).json(body);
        let resp = req
            .send()
            .await
            .map_err(|e| http::map_transport_err(CONNECTOR, e))?;
        http::ensure_success(CONNECTOR, resp).await
    }

    async fn decode_json<T: DeserializeOwned>(resp: reqwest::Response) -> Result<T> {
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| http::map_transport_err(CONNECTOR, e))?;
        serde_json::from_slice::<T>(&bytes).map_err(|e| AetherError::Permanent {
            connector: CONNECTOR,
            message: format!("failed to decode chroma response: {e}"),
        })
    }

    async fn resolve_spec(&self, ns: &ProjectId) -> Result<Option<CollectionSpec>> {
        if let Some(spec) = self
            .collections
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(ns)
            .cloned()
        {
            return Ok(Some(spec));
        }

        let name = self.collection_name(ns);
        let url = self.collection_by_name_url(&name);
        let resp = self
            .apply_auth(self.client.get(&url))
            .send()
            .await
            .map_err(|e| http::map_transport_err(CONNECTOR, e))?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let resp = http::ensure_success(CONNECTOR, resp).await?;
        let col: CollectionResponse = Self::decode_json(resp).await?;

        let meta = col.metadata.as_ref().and_then(|m| m.as_object());
        let dimensions = meta
            .and_then(|m| m.get("_aether_dimensions"))
            .and_then(|v| v.as_u64())
            .and_then(|d| usize::try_from(d).ok())
            .filter(|&d| d > 0)
            .ok_or_else(|| {
                AetherError::InvalidInput(format!(
                    "chroma collection `{name}` is missing valid `_aether_dimensions` metadata"
                ))
            })?;
        let metric = meta
            .and_then(|m| m.get("hnsw:space"))
            .and_then(|v| v.as_str())
            .and_then(hnsw_space_to_metric)
            .unwrap_or(DistanceMetric::Euclidean);

        let spec = CollectionSpec {
            id: col.id,
            dimensions,
            metric,
        };
        self.collections
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(ns.clone(), spec.clone());
        Ok(Some(spec))
    }

    async fn require_spec(&self, ns: &ProjectId) -> Result<CollectionSpec> {
        self.resolve_spec(ns).await?.ok_or_else(|| {
            AetherError::InvalidInput(format!(
                "collection for project `{ns}` is not initialized; call initialize first"
            ))
        })
    }
}

#[derive(Serialize)]
struct CreateCollectionRequest<'a> {
    name: &'a str,
    get_or_create: bool,
    metadata: Value,
}

#[derive(Deserialize)]
struct CollectionResponse {
    id: String,
    #[serde(default)]
    metadata: Option<Value>,
}

#[derive(Serialize)]
struct UpsertRequest<'a> {
    ids: Vec<String>,
    embeddings: Vec<&'a [f32]>,
    metadatas: Vec<Value>,
}

#[derive(Serialize)]
struct DeleteRequest {
    ids: Vec<String>,
}

#[derive(Serialize)]
struct QueryRequest<'a> {
    query_embeddings: [&'a [f32]; 1],
    n_results: usize,
    #[serde(rename = "where", skip_serializing_if = "Option::is_none")]
    where_clause: Option<Value>,
    include: [&'static str; 2],
}

#[derive(Deserialize)]
struct QueryResponse {
    #[serde(default)]
    ids: Vec<Vec<String>>,
    #[serde(default)]
    distances: Option<Vec<Vec<Option<f32>>>>,
    #[serde(default)]
    metadatas: Option<Vec<Vec<Option<Value>>>>,
}

#[async_trait]
impl OutboundConnector for ChromaVectorStore {
    fn name(&self) -> &'static str {
        CONNECTOR
    }

    fn capabilities(&self) -> ConnectorCapabilities {
        ConnectorCapabilities {
            max_batch_size: DEFAULT_MAX_BATCH_SIZE,
            supports_temporal: false,
            supports_filter_expr: true,
        }
    }

    async fn health_check(&self) -> Result<ConnectorHealth> {
        let url = format!("{}/api/v2/heartbeat", self.base_url);
        let resp = self
            .apply_auth(self.client.get(url))
            .send()
            .await
            .map_err(|e| http::map_transport_err(CONNECTOR, e))?;
        http::ensure_success(CONNECTOR, resp).await?;
        Ok(ConnectorHealth::Healthy)
    }

    async fn close(&self) -> Result<()> {
        Ok(())
    }
}

#[async_trait]
impl VectorStore for ChromaVectorStore {
    async fn initialize(
        &self,
        ns: &ProjectId,
        dimensions: usize,
        metric: DistanceMetric,
    ) -> Result<()> {
        if dimensions == 0 {
            return Err(AetherError::InvalidInput(
                "vector dimensions must be greater than 0".to_string(),
            ));
        }

        let name = self.collection_name(ns);
        let req = CreateCollectionRequest {
            name: &name,
            get_or_create: true,
            metadata: json!({
                "hnsw:space": metric_to_hnsw_space(metric),
                "_aether_dimensions": dimensions,
            }),
        };

        let resp = self.post_json(&self.collections_url(), &req).await?;
        let created: CollectionResponse = Self::decode_json(resp).await?;

        let mut guard = self
            .collections
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.insert(
            ns.clone(),
            CollectionSpec {
                id: created.id,
                dimensions,
                metric,
            },
        );
        Ok(())
    }

    async fn describe(&self, ns: &ProjectId) -> Result<Option<IndexSpec>> {
        Ok(self.resolve_spec(ns).await?.map(|s| IndexSpec {
            dimensions: s.dimensions,
            metric: s.metric,
        }))
    }

    async fn upsert_batch(&self, ns: &ProjectId, records: &[VectorRecord]) -> Result<()> {
        let spec = self.require_spec(ns).await?;
        if records.is_empty() {
            return Ok(());
        }

        let mut ids = Vec::with_capacity(records.len());
        let mut embeddings = Vec::with_capacity(records.len());
        let mut metadatas = Vec::with_capacity(records.len());

        for rec in records {
            if rec.vector.len() != spec.dimensions {
                return Err(AetherError::InvalidInput(format!(
                    "vector dimension mismatch for record {}: expected {}, got {}",
                    rec.id,
                    spec.dimensions,
                    rec.vector.len()
                )));
            }
            if rec.vector.iter().any(|x| !x.is_finite()) {
                return Err(AetherError::InvalidInput(format!(
                    "vector for record {} contains non-finite values",
                    rec.id
                )));
            }
            let meta = match &rec.metadata {
                Value::Object(_) => rec.metadata.clone(),
                Value::Null => json!({}),
                _ => {
                    return Err(AetherError::InvalidInput(format!(
                        "metadata for record {} must be a JSON object",
                        rec.id
                    )));
                }
            };
            ids.push(rec.id.to_string());
            embeddings.push(rec.vector.as_slice());
            metadatas.push(meta);
        }

        let req = UpsertRequest {
            ids,
            embeddings,
            metadatas,
        };
        let url = self.collection_op_url(&spec.id, "upsert");
        self.post_json(&url, &req).await?;
        Ok(())
    }

    async fn delete(&self, ns: &ProjectId, ids: &[MemoryId]) -> Result<()> {
        let spec = self.require_spec(ns).await?;
        if ids.is_empty() {
            return Ok(());
        }

        let req = DeleteRequest {
            ids: ids.iter().map(ToString::to_string).collect(),
        };
        let url = self.collection_op_url(&spec.id, "delete");
        self.post_json(&url, &req).await?;
        Ok(())
    }

    async fn search(
        &self,
        ns: &ProjectId,
        query_vector: &[f32],
        top_k: usize,
        filter: Option<&FilterExpr>,
    ) -> Result<Vec<VectorMatch>> {
        let spec = self.require_spec(ns).await?;
        if query_vector.len() != spec.dimensions {
            return Err(AetherError::InvalidInput(format!(
                "query vector dimension mismatch: expected {}, got {}",
                spec.dimensions,
                query_vector.len()
            )));
        }
        if query_vector.iter().any(|x| !x.is_finite()) {
            return Err(AetherError::InvalidInput(
                "query vector contains non-finite values".to_string(),
            ));
        }
        if top_k == 0 {
            return Ok(Vec::new());
        }

        let where_clause = match filter {
            Some(expr) => match translate_filter(expr)? {
                TranslatedFilter::MatchAll => None,
                TranslatedFilter::Unsatisfiable => return Ok(Vec::new()),
                TranslatedFilter::Clause(clause) => Some(clause),
            },
            None => None,
        };

        let req = QueryRequest {
            query_embeddings: [query_vector],
            n_results: top_k,
            where_clause,
            include: ["metadatas", "distances"],
        };
        let url = self.collection_op_url(&spec.id, "query");
        let resp = self.post_json(&url, &req).await?;
        let parsed: QueryResponse = Self::decode_json(resp).await?;

        parse_query_matches(parsed, spec.metric)
    }
}

fn metric_to_hnsw_space(metric: DistanceMetric) -> &'static str {
    match metric {
        DistanceMetric::Cosine => "cosine",
        DistanceMetric::Dot => "ip",
        DistanceMetric::Euclidean => "l2",
    }
}

fn hnsw_space_to_metric(space: &str) -> Option<DistanceMetric> {
    match space {
        "cosine" => Some(DistanceMetric::Cosine),
        "ip" => Some(DistanceMetric::Dot),
        "l2" => Some(DistanceMetric::Euclidean),
        _ => None,
    }
}

fn distance_to_score(metric: DistanceMetric, distance: f32) -> f32 {
    match metric {
        DistanceMetric::Cosine | DistanceMetric::Dot => 1.0 - distance,
        DistanceMetric::Euclidean => -distance,
    }
}

fn parse_query_matches(resp: QueryResponse, metric: DistanceMetric) -> Result<Vec<VectorMatch>> {
    let ids = resp.ids.into_iter().next().unwrap_or_default();
    if ids.is_empty() {
        return Ok(Vec::new());
    }

    let distances = resp
        .distances
        .and_then(|rows| rows.into_iter().next())
        .unwrap_or_default();
    let metadatas = resp
        .metadatas
        .and_then(|rows| rows.into_iter().next())
        .unwrap_or_default();

    let mut matches = Vec::with_capacity(ids.len());
    for (idx, raw_id) in ids.into_iter().enumerate() {
        let uuid = Uuid::parse_str(&raw_id).map_err(|e| AetherError::Permanent {
            connector: CONNECTOR,
            message: format!("invalid memory id `{raw_id}` returned by chroma: {e}"),
        })?;
        let distance =
            distances
                .get(idx)
                .and_then(|d| *d)
                .ok_or_else(|| AetherError::Permanent {
                    connector: CONNECTOR,
                    message: format!("missing distance for record `{raw_id}` in chroma response"),
                })?;
        let metadata = metadatas
            .get(idx)
            .and_then(|m| m.clone())
            .unwrap_or_else(|| json!({}));

        matches.push(VectorMatch {
            id: MemoryId::from_uuid(uuid),
            score: distance_to_score(metric, distance),
            metadata,
        });
    }

    matches.sort_by(|a, b| b.score.total_cmp(&a.score));
    Ok(matches)
}

enum TranslatedFilter {
    MatchAll,
    Unsatisfiable,
    Clause(Value),
}

fn translate_filter(expr: &FilterExpr) -> Result<TranslatedFilter> {
    match expr {
        FilterExpr::Eq { field, value } => {
            let v = filter_value_to_json(value)?;
            Ok(TranslatedFilter::Clause(json!({ field: { "$eq": v } })))
        }
        FilterExpr::In { field, values } => {
            if values.is_empty() {
                return Ok(TranslatedFilter::Unsatisfiable);
            }
            let list = values
                .iter()
                .map(filter_value_to_json)
                .collect::<Result<Vec<_>>>()?;
            Ok(TranslatedFilter::Clause(json!({ field: { "$in": list } })))
        }
        FilterExpr::Range { field, gte, lte } => translate_range(field, *gte, *lte),
        FilterExpr::And(exprs) => translate_composite("$and", exprs, false),
        FilterExpr::Or(exprs) => translate_composite("$or", exprs, true),
    }
}

fn translate_range(field: &str, gte: Option<f64>, lte: Option<f64>) -> Result<TranslatedFilter> {
    let mut clauses = Vec::new();
    if let Some(min) = gte {
        if !min.is_finite() {
            return Err(AetherError::InvalidInput(
                "range filter `gte` must be finite".to_string(),
            ));
        }
        clauses.push(json!({ field: { "$gte": min } }));
    }
    if let Some(max) = lte {
        if !max.is_finite() {
            return Err(AetherError::InvalidInput(
                "range filter `lte` must be finite".to_string(),
            ));
        }
        clauses.push(json!({ field: { "$lte": max } }));
    }
    match clauses.len() {
        0 => Err(AetherError::InvalidInput(
            "range filter must specify at least one of gte or lte".to_string(),
        )),
        1 => Ok(TranslatedFilter::Clause(
            clauses.into_iter().next().unwrap(),
        )),
        _ => Ok(TranslatedFilter::Clause(json!({ "$and": clauses }))),
    }
}

fn translate_composite(op: &str, exprs: &[FilterExpr], is_or: bool) -> Result<TranslatedFilter> {
    let mut children = Vec::with_capacity(exprs.len());
    for expr in exprs {
        match translate_filter(expr)? {
            TranslatedFilter::Clause(clause) => children.push(clause),
            TranslatedFilter::MatchAll if is_or => return Ok(TranslatedFilter::MatchAll),
            TranslatedFilter::MatchAll => {}
            TranslatedFilter::Unsatisfiable if !is_or => {
                return Ok(TranslatedFilter::Unsatisfiable);
            }
            TranslatedFilter::Unsatisfiable => {}
        }
    }
    if children.is_empty() {
        return Ok(if is_or {
            TranslatedFilter::Unsatisfiable
        } else {
            TranslatedFilter::MatchAll
        });
    }
    if children.len() == 1 {
        return Ok(TranslatedFilter::Clause(
            children.into_iter().next().unwrap(),
        ));
    }
    Ok(TranslatedFilter::Clause(json!({ op: children })))
}

fn filter_value_to_json(val: &FilterValue) -> Result<Value> {
    match val {
        FilterValue::String(s) => Ok(Value::String(s.clone())),
        FilterValue::Int(i) => Ok(json!(i)),
        FilterValue::Float(f) => {
            if !f.is_finite() {
                return Err(AetherError::InvalidInput(
                    "filter float value must be finite".to_string(),
                ));
            }
            Ok(json!(f))
        }
        FilterValue::Bool(b) => Ok(Value::Bool(*b)),
    }
}
