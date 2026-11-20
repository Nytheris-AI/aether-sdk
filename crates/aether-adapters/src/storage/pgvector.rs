//! PostgreSQL pgvector `VectorStore` connector.

use std::collections::HashMap;
use std::fmt;
use std::sync::RwLock;

use async_trait::async_trait;
use muninn_aether_core::{
    redact_url_userinfo, AetherError, ConnectorCapabilities, ConnectorHealth, DistanceMetric,
    FilterExpr, FilterValue, IndexSpec, MemoryId, OutboundConnector, ProjectId, Result,
    SecretString, VectorMatch, VectorRecord, VectorStore,
};
use pgvector::Vector;
use serde_json::Value;
use sqlx::error::DatabaseError;
use sqlx::postgres::PgPoolOptions;
use sqlx::{AssertSqlSafe, PgPool, Postgres, QueryBuilder, Row};
use uuid::Uuid;

const CONNECTOR_NAME: &str = "pgvector";
const MAX_BATCH_SIZE: usize = 1000;

/// Configuration for [`PgVectorStore`].
#[derive(Clone)]
pub struct PgVectorConfig {
    pub database_url: SecretString,
    pub table_name: String,
    pub max_connections: u32,
}

impl fmt::Debug for PgVectorConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PgVectorConfig")
            .field(
                "database_url",
                &redact_url_userinfo(self.database_url.expose_secret()),
            )
            .field("table_name", &self.table_name)
            .field("max_connections", &self.max_connections)
            .finish()
    }
}

impl PgVectorConfig {
    /// Validates configuration invariants before opening a database pool.
    pub fn validate(&self) -> Result<()> {
        validate_sql_identifier(&self.table_name)?;
        if self.database_url.expose_secret().trim().is_empty() {
            return Err(AetherError::InvalidInput(
                "database_url must not be empty".into(),
            ));
        }
        if self.max_connections == 0 {
            return Err(AetherError::InvalidInput(
                "max_connections must be greater than 0".into(),
            ));
        }
        Ok(())
    }
}

/// Validates that `ident` matches `[a-zA-Z_][a-zA-Z0-9_]*` to prevent SQL injection.
pub fn validate_sql_identifier(ident: &str) -> Result<()> {
    let mut chars = ident.chars();
    let Some(first) = chars.next() else {
        return Err(AetherError::InvalidInput(
            "SQL identifier must not be empty".into(),
        ));
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return Err(AetherError::InvalidInput(format!(
            "invalid SQL identifier `{ident}`: must match [a-zA-Z_][a-zA-Z0-9_]*"
        )));
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(AetherError::InvalidInput(format!(
            "invalid SQL identifier `{ident}`: must match [a-zA-Z_][a-zA-Z0-9_]*"
        )));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy)]
struct NamespaceSpec {
    dimensions: usize,
    metric: DistanceMetric,
}

/// PostgreSQL + pgvector implementation of [`VectorStore`].
pub struct PgVectorStore {
    pool: PgPool,
    table_name: String,
    specs: RwLock<HashMap<ProjectId, NamespaceSpec>>,
}

impl fmt::Debug for PgVectorStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PgVectorStore")
            .field("table_name", &self.table_name)
            .finish_non_exhaustive()
    }
}

impl PgVectorStore {
    /// Connects eagerly to PostgreSQL using `config` and validates the connection pool.
    pub async fn connect(config: PgVectorConfig) -> Result<Self> {
        config.validate()?;
        let pool = PgPoolOptions::new()
            .max_connections(config.max_connections)
            .connect(config.database_url.expose_secret())
            .await
            .map_err(map_sqlx_error)?;
        Ok(Self {
            pool,
            table_name: config.table_name,
            specs: RwLock::new(HashMap::new()),
        })
    }

    /// Creates a store backed by a lazily connected `PgPool` without immediate network I/O.
    ///
    /// Must be called within an active Tokio runtime because `sqlx` spawns a pool maintenance task.
    pub fn connect_lazy(config: PgVectorConfig) -> Result<Self> {
        config.validate()?;
        let pool = PgPoolOptions::new()
            .max_connections(config.max_connections)
            .connect_lazy(config.database_url.expose_secret())
            .map_err(map_sqlx_error)?;
        Ok(Self {
            pool,
            table_name: config.table_name,
            specs: RwLock::new(HashMap::new()),
        })
    }

    /// Wraps an existing `PgPool` after validating `table_name`.
    pub fn from_pool(pool: PgPool, table_name: impl Into<String>) -> Result<Self> {
        let table_name = table_name.into();
        validate_sql_identifier(&table_name)?;
        Ok(Self {
            pool,
            table_name,
            specs: RwLock::new(HashMap::new()),
        })
    }

    pub fn table_name(&self) -> &str {
        &self.table_name
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    fn specs_table(&self) -> String {
        format!("{}_specs", self.table_name)
    }

    fn namespace_spec(&self, ns: &ProjectId) -> Option<NamespaceSpec> {
        self.specs
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(ns)
            .copied()
    }

    async fn resolve_spec(&self, ns: &ProjectId) -> Result<Option<NamespaceSpec>> {
        if let Some(spec) = self.namespace_spec(ns) {
            return Ok(Some(spec));
        }
        let sql = format!(
            "SELECT dimensions, metric FROM {} WHERE ns = $1",
            self.specs_table()
        );
        let row = match sqlx::query(AssertSqlSafe(sql))
            .bind(ns.as_str())
            .fetch_optional(&self.pool)
            .await
        {
            Ok(r) => r,
            Err(sqlx::Error::Database(ref db_err)) if db_err.code().as_deref() == Some("42P01") => {
                return Ok(None);
            }
            Err(err) => return Err(map_sqlx_error(err)),
        };

        let Some(row) = row else {
            return Ok(None);
        };
        let dims_i32: i32 = row.try_get("dimensions").map_err(map_sqlx_error)?;
        let dimensions = usize::try_from(dims_i32).map_err(|_| AetherError::Permanent {
            connector: CONNECTOR_NAME,
            message: format!("invalid persisted vector dimensions: {dims_i32}"),
        })?;
        let metric_str: String = row.try_get("metric").map_err(map_sqlx_error)?;
        let metric = str_to_metric(&metric_str).ok_or_else(|| AetherError::Permanent {
            connector: CONNECTOR_NAME,
            message: format!("unknown persisted distance metric `{metric_str}`"),
        })?;
        let spec = NamespaceSpec { dimensions, metric };
        self.specs
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(ns.clone(), spec);
        Ok(Some(spec))
    }

    async fn upsert_chunk<'e, E>(
        &self,
        executor: E,
        ns: &ProjectId,
        chunk: &[&VectorRecord],
    ) -> Result<()>
    where
        E: sqlx::Executor<'e, Database = Postgres>,
    {
        let mut qb = QueryBuilder::<Postgres>::new(format!(
            "INSERT INTO {} (ns, id, embedding, metadata) ",
            self.table_name
        ));
        qb.push_values(chunk, |mut b, rec| {
            b.push_bind(ns.as_str())
                .push_bind(rec.id.0)
                .push_bind(Vector::from(rec.vector.clone()))
                .push_bind(sqlx::types::Json(&rec.metadata));
        });
        qb.push(
            " ON CONFLICT (ns, id) DO UPDATE SET \
             embedding = EXCLUDED.embedding, \
             metadata = EXCLUDED.metadata",
        );
        qb.build().execute(executor).await.map_err(map_sqlx_error)?;
        Ok(())
    }
}

#[async_trait]
impl OutboundConnector for PgVectorStore {
    fn name(&self) -> &'static str {
        CONNECTOR_NAME
    }

    fn capabilities(&self) -> ConnectorCapabilities {
        ConnectorCapabilities {
            max_batch_size: MAX_BATCH_SIZE,
            supports_temporal: false,
            supports_filter_expr: true,
        }
    }

    async fn health_check(&self) -> Result<ConnectorHealth> {
        match sqlx::query("SELECT 1").execute(&self.pool).await {
            Ok(_) => Ok(ConnectorHealth::Healthy),
            Err(err) => Ok(ConnectorHealth::Unhealthy(err.to_string())),
        }
    }

    async fn close(&self) -> Result<()> {
        self.pool.close().await;
        Ok(())
    }
}

#[async_trait]
impl VectorStore for PgVectorStore {
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
        if u16::try_from(dimensions).is_err() {
            return Err(AetherError::InvalidInput(format!(
                "vector dimensions {dimensions} exceed pgvector u16 limit"
            )));
        }

        sqlx::query("CREATE EXTENSION IF NOT EXISTS vector")
            .execute(&self.pool)
            .await
            .map_err(map_sqlx_error)?;

        let create_table = format!(
            "CREATE TABLE IF NOT EXISTS {} (\
                 ns TEXT NOT NULL, \
                 id UUID NOT NULL, \
                 embedding vector NOT NULL, \
                 metadata JSONB NOT NULL, \
                 PRIMARY KEY (ns, id)\
             )",
            self.table_name
        );
        sqlx::query(AssertSqlSafe(create_table))
            .execute(&self.pool)
            .await
            .map_err(map_sqlx_error)?;

        let create_specs = format!(
            "CREATE TABLE IF NOT EXISTS {} (\
                 ns TEXT PRIMARY KEY, \
                 dimensions INTEGER NOT NULL, \
                 metric TEXT NOT NULL\
             )",
            self.specs_table()
        );
        sqlx::query(AssertSqlSafe(create_specs))
            .execute(&self.pool)
            .await
            .map_err(map_sqlx_error)?;

        let metric_str = metric_to_str(metric);
        let dims_i32 = i32::try_from(dimensions).map_err(|_| {
            AetherError::InvalidInput(format!("vector dimensions {dimensions} exceed i32 limit"))
        })?;
        let insert_spec = format!(
            "INSERT INTO {} (ns, dimensions, metric) VALUES ($1, $2, $3) ON CONFLICT (ns) DO NOTHING",
            self.specs_table()
        );
        sqlx::query(AssertSqlSafe(insert_spec))
            .bind(ns.as_str())
            .bind(dims_i32)
            .bind(metric_str)
            .execute(&self.pool)
            .await
            .map_err(map_sqlx_error)?;

        let select_spec = format!(
            "SELECT dimensions, metric FROM {} WHERE ns = $1",
            self.specs_table()
        );
        let row = sqlx::query(AssertSqlSafe(select_spec))
            .bind(ns.as_str())
            .fetch_one(&self.pool)
            .await
            .map_err(map_sqlx_error)?;
        let existing_dims: i32 = row.try_get("dimensions").map_err(map_sqlx_error)?;
        let existing_metric_str: String = row.try_get("metric").map_err(map_sqlx_error)?;
        if existing_dims != dims_i32 || existing_metric_str != metric_str {
            return Err(AetherError::InvalidInput(format!(
                "namespace `{ns}` already initialized with dimensions={existing_dims}, metric={existing_metric_str}; requested dimensions={dimensions}, metric={metric_str}"
            )));
        }

        let mut guard = self.specs.write().unwrap_or_else(|e| e.into_inner());
        guard.insert(ns.clone(), NamespaceSpec { dimensions, metric });
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

        for rec in records {
            validate_vector(&rec.vector, None)?;
        }
        let expected_dim = self.resolve_spec(ns).await?.map(|s| s.dimensions);
        for rec in records {
            validate_vector(&rec.vector, expected_dim)?;
        }

        let deduped = deduplicate_by_last_id(records);
        if deduped.len() <= MAX_BATCH_SIZE {
            return self.upsert_chunk(&self.pool, ns, &deduped).await;
        }

        let mut tx = self.pool.begin().await.map_err(map_sqlx_error)?;
        for chunk in deduped.chunks(MAX_BATCH_SIZE) {
            self.upsert_chunk(&mut *tx, ns, chunk).await?;
        }
        tx.commit().await.map_err(map_sqlx_error)?;
        Ok(())
    }

    async fn delete(&self, ns: &ProjectId, ids: &[MemoryId]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }

        let uuids: Vec<Uuid> = ids.iter().map(|id| id.0).collect();
        let sql = format!(
            "DELETE FROM {} WHERE ns = $1 AND id = ANY($2)",
            self.table_name
        );
        sqlx::query(AssertSqlSafe(sql))
            .bind(ns.as_str())
            .bind(&uuids)
            .execute(&self.pool)
            .await
            .map_err(map_sqlx_error)?;
        Ok(())
    }

    async fn search(
        &self,
        ns: &ProjectId,
        query_vector: &[f32],
        top_k: usize,
        filter: Option<&FilterExpr>,
    ) -> Result<Vec<VectorMatch>> {
        validate_vector(query_vector, None)?;
        let spec = self.resolve_spec(ns).await?;
        validate_vector(query_vector, spec.map(|s| s.dimensions))?;
        let metric = spec.map_or(DistanceMetric::Cosine, |s| s.metric);

        let mut qb = build_search_query(&self.table_name, ns, query_vector, top_k, metric, filter)?;
        if top_k == 0 {
            return Ok(Vec::new());
        }

        let rows = qb
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(map_sqlx_error)?;

        let mut matches = Vec::with_capacity(rows.len());
        for row in rows {
            let id: Uuid = row.try_get("id").map_err(map_sqlx_error)?;
            let score: f64 = row.try_get("score").map_err(map_sqlx_error)?;
            let metadata: sqlx::types::Json<Value> =
                row.try_get("metadata").map_err(map_sqlx_error)?;
            matches.push(VectorMatch {
                id: MemoryId::from_uuid(id),
                score: if score.is_nan() { 0.0 } else { score as f32 },
                metadata: metadata.0,
            });
        }
        Ok(matches)
    }
}

fn metric_to_str(metric: DistanceMetric) -> &'static str {
    match metric {
        DistanceMetric::Cosine => "cosine",
        DistanceMetric::Dot => "dot",
        DistanceMetric::Euclidean => "euclidean",
    }
}

fn str_to_metric(raw: &str) -> Option<DistanceMetric> {
    match raw {
        "cosine" => Some(DistanceMetric::Cosine),
        "dot" => Some(DistanceMetric::Dot),
        "euclidean" => Some(DistanceMetric::Euclidean),
        _ => None,
    }
}

fn validate_vector(vector: &[f32], expected_dim: Option<usize>) -> Result<()> {
    if let Some(dim) = expected_dim {
        if vector.len() != dim {
            return Err(AetherError::InvalidInput(format!(
                "vector dimension mismatch: expected {dim}, got {}",
                vector.len()
            )));
        }
    }
    if vector.is_empty() {
        return Err(AetherError::InvalidInput(
            "vector must have at least 1 dimension".into(),
        ));
    }
    if u16::try_from(vector.len()).is_err() {
        return Err(AetherError::InvalidInput(format!(
            "vector dimensions {} exceed pgvector u16 limit",
            vector.len()
        )));
    }
    if vector.iter().any(|v| !v.is_finite()) {
        return Err(AetherError::InvalidInput(
            "vector elements must be finite f32 values".into(),
        ));
    }
    Ok(())
}

/// Retains only the last record per `MemoryId` so `ON CONFLICT DO UPDATE` never updates a key twice in one statement.
fn deduplicate_by_last_id(records: &[VectorRecord]) -> Vec<&VectorRecord> {
    if records.len() <= 1 {
        return records.iter().collect();
    }
    let mut last_index = HashMap::with_capacity(records.len());
    for (idx, rec) in records.iter().enumerate() {
        last_index.insert(rec.id, idx);
    }
    records
        .iter()
        .enumerate()
        .filter(|(idx, rec)| last_index.get(&rec.id) == Some(idx))
        .map(|(_, rec)| rec)
        .collect()
}

struct MetricSqlSpec {
    operator: &'static str,
    score_prefix: &'static str,
    score_suffix: &'static str,
}

fn metric_sql_spec(metric: DistanceMetric) -> MetricSqlSpec {
    match metric {
        DistanceMetric::Cosine => MetricSqlSpec {
            operator: "<=>",
            score_prefix: "(1.0 - (embedding <=> ",
            score_suffix: "))::double precision AS score",
        },
        DistanceMetric::Dot => MetricSqlSpec {
            operator: "<#>",
            score_prefix: "(-(embedding <#> ",
            score_suffix: "))::double precision AS score",
        },
        DistanceMetric::Euclidean => MetricSqlSpec {
            operator: "<->",
            score_prefix: "(-(embedding <-> ",
            score_suffix: "))::double precision AS score",
        },
    }
}

/// Builds a parameterized pgvector similarity search query for `table_name`.
pub fn build_search_query(
    table_name: &str,
    ns: &ProjectId,
    query_vector: &[f32],
    top_k: usize,
    metric: DistanceMetric,
    filter: Option<&FilterExpr>,
) -> Result<QueryBuilder<Postgres>> {
    validate_sql_identifier(table_name)?;
    let spec = metric_sql_spec(metric);
    let vec_param = Vector::from(query_vector.to_vec());
    let limit = i64::try_from(top_k).unwrap_or(i64::MAX);

    let mut qb = QueryBuilder::<Postgres>::new("SELECT id, ");
    qb.push(spec.score_prefix);
    qb.push_bind(vec_param.clone());
    qb.push(spec.score_suffix);
    qb.push(format_args!(", metadata FROM {table_name} WHERE ns = "));
    qb.push_bind(ns.as_str().to_owned());

    if let Some(expr) = filter {
        qb.push(" AND ");
        push_filter_expr(&mut qb, expr)?;
    }

    qb.push(format_args!(" ORDER BY embedding {} ", spec.operator));
    qb.push_bind(vec_param);
    qb.push(" ASC LIMIT ");
    qb.push_bind(limit);
    Ok(qb)
}

/// Appends a parameterized SQL predicate for `filter` onto `qb`.
pub fn push_filter_expr(qb: &mut QueryBuilder<Postgres>, filter: &FilterExpr) -> Result<()> {
    match filter {
        FilterExpr::Eq { field, value } => push_eq_predicate(qb, field, value),
        FilterExpr::In { field, values } => {
            validate_sql_identifier(field)?;
            if values.is_empty() {
                qb.push("FALSE");
                return Ok(());
            }
            qb.push("(");
            for (idx, value) in values.iter().enumerate() {
                if idx > 0 {
                    qb.push(" OR ");
                }
                push_eq_predicate(qb, field, value)?;
            }
            qb.push(")");
            Ok(())
        }
        FilterExpr::Range { field, gte, lte } => push_range_predicate(qb, field, *gte, *lte),
        FilterExpr::And(exprs) => push_combinator(qb, exprs, " AND ", "TRUE"),
        FilterExpr::Or(exprs) => push_combinator(qb, exprs, " OR ", "FALSE"),
    }
}

fn push_combinator(
    qb: &mut QueryBuilder<Postgres>,
    exprs: &[FilterExpr],
    separator: &'static str,
    empty_literal: &'static str,
) -> Result<()> {
    if exprs.is_empty() {
        qb.push(empty_literal);
        return Ok(());
    }
    qb.push("(");
    for (idx, expr) in exprs.iter().enumerate() {
        if idx > 0 {
            qb.push(separator);
        }
        push_filter_expr(qb, expr)?;
    }
    qb.push(")");
    Ok(())
}

fn push_eq_predicate(
    qb: &mut QueryBuilder<Postgres>,
    field: &str,
    value: &FilterValue,
) -> Result<()> {
    validate_sql_identifier(field)?;
    if let FilterValue::Float(num) = value {
        if !num.is_finite() {
            return Err(AetherError::InvalidInput(
                "filter float value must be finite".into(),
            ));
        }
    }

    qb.push("metadata -> ");
    qb.push_bind(field.to_owned());
    qb.push("::text = to_jsonb(");
    match value {
        FilterValue::String(s) => {
            qb.push_bind(s.clone());
            qb.push("::text)");
        }
        FilterValue::Int(i) => {
            qb.push_bind(*i);
            qb.push("::bigint)");
        }
        FilterValue::Float(f) => {
            qb.push_bind(*f);
            qb.push("::double precision)");
        }
        FilterValue::Bool(b) => {
            qb.push_bind(*b);
            qb.push("::boolean)");
        }
    }
    Ok(())
}

fn push_range_predicate(
    qb: &mut QueryBuilder<Postgres>,
    field: &str,
    gte: Option<f64>,
    lte: Option<f64>,
) -> Result<()> {
    validate_sql_identifier(field)?;
    for bound in [gte, lte].into_iter().flatten() {
        if !bound.is_finite() {
            return Err(AetherError::InvalidInput(
                "filter range bound must be finite".into(),
            ));
        }
    }

    if gte.is_none() && lte.is_none() {
        qb.push("jsonb_typeof(metadata -> ");
        qb.push_bind(field.to_owned());
        qb.push("::text) = 'number'");
        return Ok(());
    }

    // Wrap the numeric cast in CASE so non-numeric JSON fields in the same table never trigger a cast error.
    qb.push("(CASE WHEN jsonb_typeof(metadata -> ");
    qb.push_bind(field.to_owned());
    qb.push("::text) = 'number' THEN (metadata ->> ");
    qb.push_bind(field.to_owned());
    qb.push("::text)::double precision END)");

    match (gte, lte) {
        (Some(min), Some(max)) => {
            qb.push(" BETWEEN ");
            qb.push_bind(min);
            qb.push(" AND ");
            qb.push_bind(max);
        }
        (Some(min), None) => {
            qb.push(" >= ");
            qb.push_bind(min);
        }
        (None, Some(max)) => {
            qb.push(" <= ");
            qb.push_bind(max);
        }
        (None, None) => unreachable!("handled above"),
    }
    Ok(())
}

/// Maps `sqlx::Error` to retryable `AetherError::Transient` or non-retryable `AetherError::Permanent`.
pub fn map_sqlx_error(err: sqlx::Error) -> AetherError {
    let message = err.to_string();
    let is_transient = match &err {
        sqlx::Error::PoolTimedOut | sqlx::Error::Io(_) | sqlx::Error::WorkerCrashed => true,
        sqlx::Error::Database(db_err) => is_transient_database_error(db_err.as_ref()),
        _ => false,
    };

    if is_transient {
        AetherError::Transient {
            connector: CONNECTOR_NAME,
            message,
        }
    } else {
        AetherError::Permanent {
            connector: CONNECTOR_NAME,
            message,
        }
    }
}

fn is_transient_database_error(db_err: &dyn DatabaseError) -> bool {
    if db_err.is_transient_in_connect_phase() {
        return true;
    }
    let Some(code) = db_err.code() else {
        return false;
    };
    let code = code.as_ref();
    code.starts_with("08")
        || code.starts_with("40")
        || code.starts_with("53")
        || matches!(code, "57014" | "57P01" | "57P02" | "57P03")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn upsert_and_search_validate_initialized_dimensions_before_db_io() {
        let store = PgVectorStore::connect_lazy(PgVectorConfig {
            database_url: SecretString::from("postgres://user:pass@127.0.0.1:1/aether"),
            table_name: "aether_vectors".into(),
            max_connections: 1,
        })
        .unwrap();
        let ns = ProjectId::new("proj-dim").unwrap();
        store.specs.write().unwrap().insert(
            ns.clone(),
            NamespaceSpec {
                dimensions: 3,
                metric: DistanceMetric::Cosine,
            },
        );

        let bad_rec = VectorRecord {
            id: MemoryId::new(),
            vector: vec![1.0, 0.0],
            metadata: Value::Null,
        };
        let err = store
            .upsert_batch(&ns, &[bad_rec])
            .await
            .expect_err("dimension mismatch must fail");
        assert!(matches!(err, AetherError::InvalidInput(_)));

        let search_err = store
            .search(&ns, &[1.0, 0.0], 5, None)
            .await
            .expect_err("query vector dimension mismatch must fail");
        assert!(matches!(search_err, AetherError::InvalidInput(_)));

        let nan_rec = VectorRecord {
            id: MemoryId::new(),
            vector: vec![1.0, f32::NAN, 0.0],
            metadata: Value::Null,
        };
        let nan_err = store
            .upsert_batch(&ns, &[nan_rec])
            .await
            .expect_err("NaN vector must fail");
        assert!(matches!(nan_err, AetherError::InvalidInput(_)));
    }

    #[test]
    fn deduplicate_keeps_last_record_for_duplicate_id() {
        let id1 = MemoryId::new();
        let id2 = MemoryId::new();
        let recs = vec![
            VectorRecord {
                id: id1,
                vector: vec![1.0, 0.0],
                metadata: serde_json::json!({"v": 1}),
            },
            VectorRecord {
                id: id2,
                vector: vec![0.0, 1.0],
                metadata: serde_json::json!({"v": 2}),
            },
            VectorRecord {
                id: id1,
                vector: vec![0.5, 0.5],
                metadata: serde_json::json!({"v": 3}),
            },
        ];
        let deduped = deduplicate_by_last_id(&recs);
        assert_eq!(deduped.len(), 2);
        assert_eq!(deduped[0].id, id2);
        assert_eq!(deduped[1].id, id1);
        assert_eq!(deduped[1].metadata["v"], 3);
    }
}
