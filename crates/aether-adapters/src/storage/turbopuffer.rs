//! Turbopuffer `VectorStore` connector.

use std::collections::HashMap;
use std::sync::RwLock;

use aether_core::{
    AetherError, ConnectorCapabilities, ConnectorHealth, DistanceMetric, FilterExpr, FilterValue,
    IndexSpec, MemoryId, OutboundConnector, ProjectId, Result, SecretString, VectorMatch,
    VectorRecord, VectorStore,
};
use async_trait::async_trait;
use serde::Serialize;
use serde_json::{json, Map, Value};
use uuid::Uuid;

use crate::storage::http;

const CONNECTOR_NAME: &str = "turbopuffer";
const DEFAULT_MAX_BATCH_SIZE: usize = 1000;

/// Configuration for [`TurbopufferVectorStore`].
#[derive(Debug, Clone)]
pub struct TurbopufferConfig {
    pub base_url: String,
    pub api_key: SecretString,
    pub namespace_prefix: String,
}

#[derive(Debug, Clone, Copy)]
struct NamespaceSpec {
    dimensions: usize,
    metric: DistanceMetric,
}

/// HTTP-backed [`VectorStore`] connector for Turbopuffer (`/v1/namespaces`).
#[derive(Debug)]
pub struct TurbopufferVectorStore {
    client: reqwest::Client,
    base_url: String,
    api_key: SecretString,
    namespace_prefix: String,
    specs: RwLock<HashMap<ProjectId, NamespaceSpec>>,
}

#[derive(Serialize)]
struct UpsertRequest {
    upsert_rows: Vec<Value>,
    distance_metric: &'static str,
}

#[derive(Serialize)]
struct DeleteRequest {
    deletes: Vec<String>,
}

#[derive(Serialize)]
struct QueryRequest<'a> {
    rank_by: (&'static str, &'static str, &'a [f32]),
    top_k: usize,
    include_attributes: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    filters: Option<Value>,
}

impl TurbopufferVectorStore {
    /// Validates configuration and constructs a [`TurbopufferVectorStore`].
    pub fn new(config: TurbopufferConfig) -> Result<Self> {
        let base_url = http::normalize_base_url(&config.base_url)?;
        let client = reqwest::Client::builder()
            .build()
            .map_err(|err| AetherError::Permanent {
                connector: CONNECTOR_NAME,
                message: format!("failed to build HTTP client: {err}"),
            })?;

        Ok(Self {
            client,
            base_url,
            api_key: config.api_key,
            namespace_prefix: config.namespace_prefix,
            specs: RwLock::new(HashMap::new()),
        })
    }

    fn namespace_name(&self, ns: &ProjectId) -> String {
        format!("{}{}", self.namespace_prefix, ns.as_str())
    }

    fn spec_for(&self, ns: &ProjectId) -> Option<NamespaceSpec> {
        self.specs.read().unwrap().get(ns).copied()
    }

    async fn send_post<T: Serialize>(&self, path: &str, body: &T) -> Result<reqwest::Response> {
        let url = format!("{}{path}", self.base_url);
        let resp = self
            .client
            .post(&url)
            .bearer_auth(self.api_key.expose_secret())
            .json(body)
            .send()
            .await
            .map_err(|err| http::map_transport_err(CONNECTOR_NAME, err))?;
        http::ensure_success(CONNECTOR_NAME, resp).await
    }
}

#[async_trait]
impl OutboundConnector for TurbopufferVectorStore {
    fn name(&self) -> &'static str {
        CONNECTOR_NAME
    }

    fn capabilities(&self) -> ConnectorCapabilities {
        ConnectorCapabilities {
            max_batch_size: DEFAULT_MAX_BATCH_SIZE,
            supports_temporal: false,
            supports_filter_expr: true,
        }
    }

    async fn health_check(&self) -> Result<ConnectorHealth> {
        let url = format!("{}/v1/namespaces?page_size=1", self.base_url);
        let resp = self
            .client
            .get(&url)
            .bearer_auth(self.api_key.expose_secret())
            .send()
            .await
            .map_err(|err| http::map_transport_err(CONNECTOR_NAME, err))?;
        http::ensure_success(CONNECTOR_NAME, resp).await?;
        Ok(ConnectorHealth::Healthy)
    }

    async fn close(&self) -> Result<()> {
        Ok(())
    }
}

#[async_trait]
impl VectorStore for TurbopufferVectorStore {
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
        self.specs
            .write()
            .unwrap()
            .insert(ns.clone(), NamespaceSpec { dimensions, metric });
        Ok(())
    }

    async fn describe(&self, ns: &ProjectId) -> Result<Option<IndexSpec>> {
        Ok(self.spec_for(ns).map(|s| IndexSpec {
            dimensions: s.dimensions,
            metric: s.metric,
        }))
    }

    async fn upsert_batch(&self, ns: &ProjectId, records: &[VectorRecord]) -> Result<()> {
        if records.is_empty() {
            return Ok(());
        }

        let spec = self.spec_for(ns);
        let expected_dim = spec
            .map(|s| s.dimensions)
            .unwrap_or(records[0].vector.len());
        if expected_dim == 0 {
            return Err(AetherError::InvalidInput(
                "vector dimensions must be greater than 0".into(),
            ));
        }

        let mut upsert_rows = Vec::with_capacity(records.len());
        for rec in records {
            if rec.vector.len() != expected_dim {
                return Err(AetherError::InvalidInput(format!(
                    "vector dimension mismatch: expected {expected_dim}, got {}",
                    rec.vector.len()
                )));
            }
            validate_finite_vector(&rec.vector)?;
            upsert_rows.push(record_to_row(rec)?);
        }

        let metric = spec.map_or_else(DistanceMetric::default, |s| s.metric);
        let body = UpsertRequest {
            upsert_rows,
            distance_metric: distance_metric_name(metric),
        };
        let path = format!("/v1/namespaces/{}", self.namespace_name(ns));
        self.send_post(&path, &body).await?;
        Ok(())
    }

    async fn delete(&self, ns: &ProjectId, ids: &[MemoryId]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }

        let body = DeleteRequest {
            deletes: ids.iter().map(ToString::to_string).collect(),
        };
        let path = format!("/v1/namespaces/{}", self.namespace_name(ns));
        self.send_post(&path, &body).await?;
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
                "query vector must not be empty".into(),
            ));
        }
        validate_finite_vector(query_vector)?;

        let spec = self.spec_for(ns);
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

        let filters = filter.map(translate_filter).transpose()?;
        let body = QueryRequest {
            rank_by: ("vector", "ANN", query_vector),
            top_k,
            include_attributes: true,
            filters,
        };
        let path = format!("/v1/namespaces/{}/query", self.namespace_name(ns));
        let resp = self.send_post(&path, &body).await?;
        let bytes = resp
            .bytes()
            .await
            .map_err(|err| http::map_transport_err(CONNECTOR_NAME, err))?;
        let raw: Value = serde_json::from_slice(&bytes)?;
        let metric = spec.map_or_else(DistanceMetric::default, |s| s.metric);
        parse_query_response(raw, metric)
    }
}

fn distance_metric_name(metric: DistanceMetric) -> &'static str {
    match metric {
        DistanceMetric::Cosine => "cosine_distance",
        DistanceMetric::Dot => "dot_product",
        DistanceMetric::Euclidean => "euclidean_squared",
    }
}

fn normalize_score(metric: DistanceMetric, dist: f32) -> f32 {
    match metric {
        DistanceMetric::Cosine => 1.0 - dist,
        DistanceMetric::Dot => -dist,
        DistanceMetric::Euclidean => -dist.max(0.0).sqrt(),
    }
}

fn validate_finite_vector(vector: &[f32]) -> Result<()> {
    if vector.iter().any(|v| !v.is_finite()) {
        return Err(AetherError::InvalidInput(
            "vector components must be finite numbers".into(),
        ));
    }
    Ok(())
}

fn record_to_row(rec: &VectorRecord) -> Result<Value> {
    let mut row = Map::new();
    match &rec.metadata {
        Value::Object(map) => {
            for (k, v) in map {
                if k != "id" && k != "vector" {
                    row.insert(k.clone(), v.clone());
                }
            }
        }
        Value::Null => {}
        _ => {
            return Err(AetherError::InvalidInput(
                "vector metadata must be a JSON object or null".into(),
            ));
        }
    }
    row.insert("id".into(), Value::String(rec.id.to_string()));
    row.insert("vector".into(), serde_json::to_value(&rec.vector)?);
    Ok(Value::Object(row))
}

fn filter_value_to_json(val: &FilterValue) -> Result<Value> {
    match val {
        FilterValue::String(s) => Ok(Value::String(s.clone())),
        FilterValue::Int(i) => Ok(json!(i)),
        FilterValue::Float(f) => {
            if !f.is_finite() {
                return Err(AetherError::InvalidInput(
                    "filter float value must be finite".into(),
                ));
            }
            Ok(json!(f))
        }
        FilterValue::Bool(b) => Ok(Value::Bool(*b)),
    }
}

fn translate_filter(expr: &FilterExpr) -> Result<Value> {
    match expr {
        FilterExpr::Eq { field, value } => {
            validate_field_name(field)?;
            Ok(json!([field, "Eq", filter_value_to_json(value)?]))
        }
        FilterExpr::In { field, values } => {
            validate_field_name(field)?;
            let encoded = values
                .iter()
                .map(filter_value_to_json)
                .collect::<Result<Vec<_>>>()?;
            Ok(json!([field, "In", encoded]))
        }
        FilterExpr::Range { field, gte, lte } => {
            validate_field_name(field)?;
            let mut clauses = Vec::with_capacity(2);
            if let Some(min) = gte {
                if !min.is_finite() {
                    return Err(AetherError::InvalidInput(
                        "range gte bound must be finite".into(),
                    ));
                }
                clauses.push(json!([field, "Gte", min]));
            }
            if let Some(max) = lte {
                if !max.is_finite() {
                    return Err(AetherError::InvalidInput(
                        "range lte bound must be finite".into(),
                    ));
                }
                clauses.push(json!([field, "Lte", max]));
            }
            if clauses.len() == 1 {
                Ok(clauses.pop().unwrap())
            } else {
                Ok(json!(["And", clauses]))
            }
        }
        FilterExpr::And(exprs) => {
            let children = exprs
                .iter()
                .map(translate_filter)
                .collect::<Result<Vec<_>>>()?;
            Ok(json!(["And", children]))
        }
        FilterExpr::Or(exprs) => {
            let children = exprs
                .iter()
                .map(translate_filter)
                .collect::<Result<Vec<_>>>()?;
            Ok(json!(["Or", children]))
        }
    }
}

fn validate_field_name(field: &str) -> Result<()> {
    if field.trim().is_empty() {
        return Err(AetherError::InvalidInput(
            "filter field must not be empty".into(),
        ));
    }
    Ok(())
}

fn parse_query_response(raw: Value, metric: DistanceMetric) -> Result<Vec<VectorMatch>> {
    let rows = match raw {
        Value::Array(items) => items,
        Value::Object(mut obj) => match obj.remove("rows") {
            Some(Value::Array(items)) => items,
            _ => {
                return Err(AetherError::Permanent {
                    connector: CONNECTOR_NAME,
                    message: "turbopuffer query response missing rows array".into(),
                });
            }
        },
        _ => {
            return Err(AetherError::Permanent {
                connector: CONNECTOR_NAME,
                message: "turbopuffer query response must be a JSON array or object".into(),
            });
        }
    };

    let mut matches = Vec::with_capacity(rows.len());
    for row in rows {
        matches.push(parse_query_row(row, metric)?);
    }

    matches.sort_by(|a, b| b.score.total_cmp(&a.score));
    Ok(matches)
}

fn parse_query_row(row: Value, metric: DistanceMetric) -> Result<VectorMatch> {
    let mut obj = match row {
        Value::Object(map) => map,
        _ => {
            return Err(AetherError::Permanent {
                connector: CONNECTOR_NAME,
                message: "turbopuffer query row must be a JSON object".into(),
            });
        }
    };

    let id_str = obj
        .remove("id")
        .and_then(|v| match v {
            Value::String(s) => Some(s),
            _ => None,
        })
        .ok_or_else(|| AetherError::Permanent {
            connector: CONNECTOR_NAME,
            message: "turbopuffer query row missing string id".into(),
        })?;
    let uuid = Uuid::parse_str(&id_str).map_err(|err| AetherError::Permanent {
        connector: CONNECTOR_NAME,
        message: format!("invalid MemoryId UUID `{id_str}`: {err}"),
    })?;

    let dist_val = obj
        .remove("dist")
        .or_else(|| obj.remove("$dist"))
        .and_then(|v| v.as_f64())
        .ok_or_else(|| AetherError::Permanent {
            connector: CONNECTOR_NAME,
            message: "turbopuffer query row missing numeric dist".into(),
        })?;

    let metadata = match obj.remove("attributes") {
        Some(Value::Object(attrs)) => Value::Object(attrs),
        Some(Value::Null) => Value::Object(Map::new()),
        Some(_) => {
            return Err(AetherError::Permanent {
                connector: CONNECTOR_NAME,
                message: "turbopuffer query row attributes must be an object".into(),
            });
        }
        None => {
            obj.remove("vector");
            Value::Object(obj)
        }
    };

    Ok(VectorMatch {
        id: MemoryId::from_uuid(uuid),
        score: normalize_score(metric, dist_val as f32),
        metadata,
    })
}
