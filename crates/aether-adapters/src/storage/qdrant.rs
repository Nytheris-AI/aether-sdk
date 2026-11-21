//! Qdrant `VectorStore` connector.

use std::collections::HashMap;
use std::sync::RwLock;

use async_trait::async_trait;
use muninn_aether_core::{
    AetherError, ConnectorCapabilities, ConnectorHealth, DistanceMetric, FilterExpr, FilterValue,
    IndexSpec, MemoryId, OutboundConnector, ProjectId, Result, SecretString, VectorMatch,
    VectorRecord, VectorStore,
};
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::http::{ensure_success, map_transport_err, normalize_base_url};

const CONNECTOR: &str = "qdrant";
const DEFAULT_MAX_BATCH_SIZE: usize = 256;

/// Configuration for [`QdrantVectorStore`].
#[derive(Debug, Clone)]
pub struct QdrantConfig {
    pub base_url: String,
    pub api_key: Option<SecretString>,
    pub collection_prefix: String,
}

impl Default for QdrantConfig {
    fn default() -> Self {
        Self {
            base_url: "http://localhost:6333".into(),
            api_key: None,
            collection_prefix: "aether".into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CollectionSpec {
    dimensions: usize,
    metric: DistanceMetric,
}

/// Qdrant REST API implementation of [`VectorStore`].
#[derive(Debug)]
pub struct QdrantVectorStore {
    client: reqwest::Client,
    base_url: String,
    api_key: Option<SecretString>,
    collection_prefix: String,
    collections: RwLock<HashMap<ProjectId, CollectionSpec>>,
}

impl QdrantVectorStore {
    /// Creates a new Qdrant connector after validating configuration at the boundary.
    pub fn new(config: QdrantConfig) -> Result<Self> {
        let base_url = normalize_base_url(&config.base_url)?;
        let client = reqwest::Client::builder()
            .build()
            .map_err(|err| map_transport_err(CONNECTOR, err))?;
        Ok(Self {
            client,
            base_url,
            api_key: config.api_key,
            collection_prefix: config.collection_prefix.trim().to_string(),
            collections: RwLock::new(HashMap::new()),
        })
    }

    fn collection_name(&self, ns: &ProjectId) -> String {
        let sanitized: String = ns
            .as_str()
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let hash = fnv1a_64(ns.as_str().as_bytes());
        let prefix = self.collection_prefix.trim_end_matches(['_', '-']);
        if prefix.is_empty() {
            format!("{sanitized}_{hash:016x}")
        } else {
            format!("{prefix}_{sanitized}_{hash:016x}")
        }
    }

    fn request(&self, method: Method, path: &str) -> reqwest::RequestBuilder {
        let url = format!("{}{}", self.base_url, path);
        let builder = self.client.request(method, url);
        match &self.api_key {
            Some(key) => builder.header("api-key", key.expose_secret()),
            None => builder,
        }
    }

    fn spec_for(&self, ns: &ProjectId) -> Option<CollectionSpec> {
        self.collections.read().unwrap().get(ns).copied()
    }

    async fn resolve_spec(&self, ns: &ProjectId) -> Result<Option<CollectionSpec>> {
        if let Some(spec) = self.spec_for(ns) {
            return Ok(Some(spec));
        }
        let collection = self.collection_name(ns);
        let resp = self
            .request(Method::GET, &format!("/collections/{collection}"))
            .send()
            .await
            .map_err(|err| map_transport_err(CONNECTOR, err))?;
        if resp.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let resp = ensure_success(CONNECTOR, resp).await?;
        let bytes = resp
            .bytes()
            .await
            .map_err(|err| map_transport_err(CONNECTOR, err))?;
        let info: CollectionInfoResponse = serde_json::from_slice(&bytes)?;
        let existing = info.result.config.params.vectors;
        let spec = CollectionSpec {
            dimensions: existing.size,
            metric: DistanceMetric::from(existing.distance),
        };
        self.collections.write().unwrap().insert(ns.clone(), spec);
        Ok(Some(spec))
    }

    async fn verify_existing_collection(
        &self,
        ns: &ProjectId,
        collection: &str,
        dimensions: usize,
        metric: DistanceMetric,
    ) -> Result<()> {
        if let Some(spec) = self.spec_for(ns) {
            if spec.dimensions == dimensions && spec.metric == metric {
                return Ok(());
            }
            return Err(AetherError::InvalidInput(format!(
                "collection `{collection}` already initialized with dimensions={}, metric={:?}",
                spec.dimensions, spec.metric
            )));
        }

        let resp = self
            .request(Method::GET, &format!("/collections/{collection}"))
            .send()
            .await
            .map_err(|err| map_transport_err(CONNECTOR, err))?;
        let resp = ensure_success(CONNECTOR, resp).await?;
        let bytes = resp
            .bytes()
            .await
            .map_err(|err| map_transport_err(CONNECTOR, err))?;
        let info: CollectionInfoResponse = serde_json::from_slice(&bytes)?;
        let existing = info.result.config.params.vectors;
        let expected_distance = QdrantDistance::from(metric);
        if existing.size == dimensions && existing.distance == expected_distance {
            self.collections
                .write()
                .unwrap()
                .insert(ns.clone(), CollectionSpec { dimensions, metric });
            return Ok(());
        }

        Err(AetherError::InvalidInput(format!(
            "existing collection `{collection}` has size={}, distance={:?}; expected size={dimensions}, distance={expected_distance:?}",
            existing.size, existing.distance
        )))
    }
}

#[async_trait]
impl OutboundConnector for QdrantVectorStore {
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
        let resp = self
            .request(Method::GET, "/healthz")
            .send()
            .await
            .map_err(|err| map_transport_err(CONNECTOR, err))?;
        ensure_success(CONNECTOR, resp).await?;
        Ok(ConnectorHealth::Healthy)
    }

    async fn close(&self) -> Result<()> {
        Ok(())
    }
}

#[async_trait]
impl VectorStore for QdrantVectorStore {
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

        let collection = self.collection_name(ns);
        let body = CreateCollectionRequest {
            vectors: VectorParams {
                size: dimensions,
                distance: QdrantDistance::from(metric),
            },
        };

        let resp = self
            .request(Method::PUT, &format!("/collections/{collection}"))
            .json(&body)
            .send()
            .await
            .map_err(|err| map_transport_err(CONNECTOR, err))?;

        if resp.status() == StatusCode::CONFLICT {
            return self
                .verify_existing_collection(ns, &collection, dimensions, metric)
                .await;
        }

        ensure_success(CONNECTOR, resp).await?;
        self.collections
            .write()
            .unwrap()
            .insert(ns.clone(), CollectionSpec { dimensions, metric });
        Ok(())
    }

    async fn describe(&self, ns: &ProjectId) -> Result<Option<IndexSpec>> {
        Ok(self.resolve_spec(ns).await?.map(|s| IndexSpec {
            dimensions: s.dimensions,
            metric: s.metric,
        }))
    }

    async fn upsert_batch(&self, ns: &ProjectId, records: &[VectorRecord]) -> Result<()> {
        if records.is_empty() {
            return Ok(());
        }

        let expected_dim = self.resolve_spec(ns).await?.map(|spec| spec.dimensions);
        for rec in records {
            if rec.vector.is_empty() {
                return Err(AetherError::InvalidInput(format!(
                    "vector for record {} must not be empty",
                    rec.id
                )));
            }
            if rec.vector.iter().any(|x| !x.is_finite()) {
                return Err(AetherError::InvalidInput(format!(
                    "vector for record {} contains non-finite values",
                    rec.id
                )));
            }
            if let Some(dim) = expected_dim {
                if rec.vector.len() != dim {
                    return Err(AetherError::InvalidInput(format!(
                        "vector dimension mismatch for {}: expected {dim}, got {}",
                        rec.id,
                        rec.vector.len()
                    )));
                }
            }
        }

        let points: Vec<QdrantPoint<'_>> = records
            .iter()
            .map(|rec| QdrantPoint {
                id: rec.id,
                vector: &rec.vector,
                payload: if rec.metadata.is_null() {
                    Value::Object(Default::default())
                } else {
                    rec.metadata.clone()
                },
            })
            .collect();

        let collection = self.collection_name(ns);
        let resp = self
            .request(
                Method::PUT,
                &format!("/collections/{collection}/points?wait=true"),
            )
            .json(&UpsertPointsRequest { points })
            .send()
            .await
            .map_err(|err| map_transport_err(CONNECTOR, err))?;
        ensure_success(CONNECTOR, resp).await?;
        Ok(())
    }

    async fn delete(&self, ns: &ProjectId, ids: &[MemoryId]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }

        let collection = self.collection_name(ns);
        let resp = self
            .request(
                Method::POST,
                &format!("/collections/{collection}/points/delete?wait=true"),
            )
            .json(&DeletePointsRequest { points: ids })
            .send()
            .await
            .map_err(|err| map_transport_err(CONNECTOR, err))?;
        ensure_success(CONNECTOR, resp).await?;
        Ok(())
    }

    async fn search(
        &self,
        ns: &ProjectId,
        query_vector: &[f32],
        top_k: usize,
        filter: Option<&FilterExpr>,
    ) -> Result<Vec<VectorMatch>> {
        if query_vector.is_empty() {
            return Err(AetherError::InvalidInput(
                "query_vector must not be empty".into(),
            ));
        }
        if query_vector.iter().any(|x| !x.is_finite()) {
            return Err(AetherError::InvalidInput(
                "query_vector contains non-finite values".into(),
            ));
        }
        let spec = self.resolve_spec(ns).await?;
        if let Some(spec) = spec {
            if query_vector.len() != spec.dimensions {
                return Err(AetherError::InvalidInput(format!(
                    "query vector dimension mismatch: expected {}, got {}",
                    spec.dimensions,
                    query_vector.len()
                )));
            }
        }
        if top_k == 0 {
            return Ok(Vec::new());
        }

        let qdrant_filter = filter.map(translate_filter).transpose()?;
        let collection = self.collection_name(ns);
        let body = SearchRequest {
            vector: query_vector,
            limit: top_k,
            with_payload: true,
            filter: qdrant_filter,
        };

        let resp = self
            .request(
                Method::POST,
                &format!("/collections/{collection}/points/search"),
            )
            .json(&body)
            .send()
            .await
            .map_err(|err| map_transport_err(CONNECTOR, err))?;
        let resp = ensure_success(CONNECTOR, resp).await?;
        let bytes = resp
            .bytes()
            .await
            .map_err(|err| map_transport_err(CONNECTOR, err))?;
        let parsed: SearchResponse = serde_json::from_slice(&bytes)?;
        let metric = spec.map_or(DistanceMetric::Cosine, |s| s.metric);

        Ok(parsed
            .result
            .into_iter()
            .map(|hit| VectorMatch {
                id: hit.id,
                score: if metric == DistanceMetric::Euclidean {
                    -hit.score
                } else {
                    hit.score
                },
                metadata: hit.payload.unwrap_or_else(|| json!({})),
            })
            .collect())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum QdrantDistance {
    Cosine,
    Dot,
    Euclid,
}

fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

impl From<DistanceMetric> for QdrantDistance {
    fn from(metric: DistanceMetric) -> Self {
        match metric {
            DistanceMetric::Cosine => Self::Cosine,
            DistanceMetric::Dot => Self::Dot,
            DistanceMetric::Euclidean => Self::Euclid,
        }
    }
}

impl From<QdrantDistance> for DistanceMetric {
    fn from(distance: QdrantDistance) -> Self {
        match distance {
            QdrantDistance::Cosine => Self::Cosine,
            QdrantDistance::Dot => Self::Dot,
            QdrantDistance::Euclid => Self::Euclidean,
        }
    }
}

#[derive(Debug, Serialize)]
struct CreateCollectionRequest {
    vectors: VectorParams,
}

#[derive(Debug, Serialize, Deserialize)]
struct VectorParams {
    size: usize,
    distance: QdrantDistance,
}

#[derive(Debug, Deserialize)]
struct CollectionInfoResponse {
    result: CollectionInfoResult,
}

#[derive(Debug, Deserialize)]
struct CollectionInfoResult {
    config: CollectionConfig,
}

#[derive(Debug, Deserialize)]
struct CollectionConfig {
    params: CollectionParams,
}

#[derive(Debug, Deserialize)]
struct CollectionParams {
    vectors: VectorParams,
}

#[derive(Debug, Serialize)]
struct UpsertPointsRequest<'a> {
    points: Vec<QdrantPoint<'a>>,
}

#[derive(Debug, Serialize)]
struct QdrantPoint<'a> {
    id: MemoryId,
    vector: &'a [f32],
    payload: Value,
}

#[derive(Debug, Serialize)]
struct DeletePointsRequest<'a> {
    points: &'a [MemoryId],
}

#[derive(Debug, Serialize)]
struct SearchRequest<'a> {
    vector: &'a [f32],
    limit: usize,
    with_payload: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    filter: Option<QdrantFilter>,
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
    result: Vec<SearchHit>,
}

#[derive(Debug, Deserialize)]
struct SearchHit {
    id: MemoryId,
    score: f32,
    payload: Option<Value>,
}

#[derive(Debug, Serialize)]
struct QdrantFilter {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    must: Vec<QdrantCondition>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    should: Vec<QdrantCondition>,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum QdrantCondition {
    Field(QdrantFieldCondition),
    Filter(QdrantFilter),
    HasId { has_id: Vec<String> },
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum QdrantFieldCondition {
    Match {
        key: String,
        #[serde(rename = "match")]
        matcher: QdrantMatch,
    },
    Range {
        key: String,
        range: QdrantRange,
    },
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum QdrantMatch {
    Value { value: QdrantScalar },
    Any { any: QdrantAnyValues },
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum QdrantScalar {
    String(String),
    Int(i64),
    Bool(bool),
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum QdrantAnyValues {
    Strings(Vec<String>),
    Ints(Vec<i64>),
}

#[derive(Debug, Serialize)]
struct QdrantRange {
    #[serde(skip_serializing_if = "Option::is_none")]
    gte: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    lte: Option<f64>,
}

fn translate_filter(expr: &FilterExpr) -> Result<QdrantFilter> {
    match expr {
        FilterExpr::And(exprs) => Ok(QdrantFilter {
            must: exprs
                .iter()
                .map(translate_condition)
                .collect::<Result<Vec<_>>>()?,
            should: Vec::new(),
        }),
        FilterExpr::Or(exprs) => translate_or(exprs),
        _ => Ok(QdrantFilter {
            must: vec![translate_condition(expr)?],
            should: Vec::new(),
        }),
    }
}

fn translate_or(exprs: &[FilterExpr]) -> Result<QdrantFilter> {
    if exprs.is_empty() {
        // An empty `should` array is ignored by Qdrant, whereas `has_id: []` matches nothing.
        return Ok(QdrantFilter {
            must: vec![QdrantCondition::HasId { has_id: Vec::new() }],
            should: Vec::new(),
        });
    }
    Ok(QdrantFilter {
        must: Vec::new(),
        should: exprs
            .iter()
            .map(translate_condition)
            .collect::<Result<Vec<_>>>()?,
    })
}

fn translate_condition(expr: &FilterExpr) -> Result<QdrantCondition> {
    match expr {
        FilterExpr::Eq { field, value } => translate_eq(field, value),
        FilterExpr::In { field, values } => translate_in(field, values),
        FilterExpr::Range { field, gte, lte } => {
            if gte.is_none() && lte.is_none() {
                return Err(AetherError::InvalidInput(
                    "range filter requires at least one of gte or lte".into(),
                ));
            }
            if gte.is_some_and(|v| !v.is_finite()) || lte.is_some_and(|v| !v.is_finite()) {
                return Err(AetherError::InvalidInput(
                    "range filter bounds must be finite numbers".into(),
                ));
            }
            Ok(QdrantCondition::Field(QdrantFieldCondition::Range {
                key: field.clone(),
                range: QdrantRange {
                    gte: *gte,
                    lte: *lte,
                },
            }))
        }
        FilterExpr::And(exprs) => Ok(QdrantCondition::Filter(QdrantFilter {
            must: exprs
                .iter()
                .map(translate_condition)
                .collect::<Result<Vec<_>>>()?,
            should: Vec::new(),
        })),
        FilterExpr::Or(exprs) => Ok(QdrantCondition::Filter(translate_or(exprs)?)),
    }
}

fn translate_eq(field: &str, value: &FilterValue) -> Result<QdrantCondition> {
    match value {
        FilterValue::String(s) => Ok(QdrantCondition::Field(QdrantFieldCondition::Match {
            key: field.to_string(),
            matcher: QdrantMatch::Value {
                value: QdrantScalar::String(s.clone()),
            },
        })),
        FilterValue::Int(i) => Ok(QdrantCondition::Field(QdrantFieldCondition::Match {
            key: field.to_string(),
            matcher: QdrantMatch::Value {
                value: QdrantScalar::Int(*i),
            },
        })),
        FilterValue::Bool(b) => Ok(QdrantCondition::Field(QdrantFieldCondition::Match {
            key: field.to_string(),
            matcher: QdrantMatch::Value {
                value: QdrantScalar::Bool(*b),
            },
        })),
        FilterValue::Float(f) => {
            if !f.is_finite() {
                return Err(AetherError::InvalidInput(
                    "float filter value must be finite".into(),
                ));
            }
            // Qdrant `match.value` accepts string, integer, or boolean; model float equality as a point range.
            Ok(QdrantCondition::Field(QdrantFieldCondition::Range {
                key: field.to_string(),
                range: QdrantRange {
                    gte: Some(*f),
                    lte: Some(*f),
                },
            }))
        }
    }
}

fn translate_in(field: &str, values: &[FilterValue]) -> Result<QdrantCondition> {
    if values.is_empty() {
        return Ok(QdrantCondition::Field(QdrantFieldCondition::Match {
            key: field.to_string(),
            matcher: QdrantMatch::Any {
                any: QdrantAnyValues::Strings(Vec::new()),
            },
        }));
    }

    if let Some(strings) = values
        .iter()
        .map(|v| match v {
            FilterValue::String(s) => Some(s.clone()),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()
    {
        return Ok(QdrantCondition::Field(QdrantFieldCondition::Match {
            key: field.to_string(),
            matcher: QdrantMatch::Any {
                any: QdrantAnyValues::Strings(strings),
            },
        }));
    }

    if let Some(ints) = values
        .iter()
        .map(|v| match v {
            FilterValue::Int(i) => Some(*i),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()
    {
        return Ok(QdrantCondition::Field(QdrantFieldCondition::Match {
            key: field.to_string(),
            matcher: QdrantMatch::Any {
                any: QdrantAnyValues::Ints(ints),
            },
        }));
    }

    // Qdrant `match.any` only supports homogeneous string or integer arrays.
    let should = values
        .iter()
        .map(|v| translate_eq(field, v))
        .collect::<Result<Vec<_>>>()?;
    Ok(QdrantCondition::Filter(QdrantFilter {
        must: Vec::new(),
        should,
    }))
}
