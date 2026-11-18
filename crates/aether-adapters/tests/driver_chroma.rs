#![cfg(all(feature = "chroma", feature = "contract-suite"))]

use std::collections::{HashMap, VecDeque};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};

use aether_adapters::contract;
use aether_adapters::storage::chroma::{ChromaConfig, ChromaVectorStore};
use aether_core::{
    AetherError, ConnectorHealth, DistanceMetric, FilterExpr, FilterValue, MemoryId,
    OutboundConnector, ProjectId, SecretString, VectorRecord, VectorStore,
};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use uuid::Uuid;

#[derive(Clone)]
struct StoredCollection {
    id: String,
    name: String,
    space: String,
    metadata: Value,
    records: HashMap<String, (Vec<f32>, Value)>,
}

#[derive(Default)]
struct MockChromaState {
    expected_token: Option<String>,
    collections: Mutex<HashMap<(String, String, String), StoredCollection>>,
    id_index: Mutex<HashMap<String, (String, String, String)>>,
    forced_responses: Mutex<VecDeque<(StatusCode, String)>>,
    create_collection_calls: AtomicUsize,
}

impl MockChromaState {
    fn new(expected_token: Option<String>) -> Self {
        Self {
            expected_token,
            ..Default::default()
        }
    }

    fn push_forced_error(&self, status: StatusCode, body: impl Into<String>) {
        self.forced_responses
            .lock()
            .unwrap()
            .push_back((status, body.into()));
    }

    fn check_auth_and_faults(&self, headers: &HeaderMap) -> Result<(), (StatusCode, String)> {
        if let Some((status, body)) = self.forced_responses.lock().unwrap().pop_front() {
            return Err((status, body));
        }
        if let Some(expected) = &self.expected_token {
            let expected_header = format!("Bearer {expected}");
            let actual = headers
                .get(axum::http::header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok());
            if actual != Some(expected_header.as_str()) {
                return Err((StatusCode::UNAUTHORIZED, "unauthorized".into()));
            }
        }
        Ok(())
    }

    fn resolve_collection_key(
        &self,
        tenant: &str,
        database: &str,
        id: &str,
    ) -> Result<(String, String, String), (StatusCode, String)> {
        let key = self
            .id_index
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| (StatusCode::NOT_FOUND, "collection not found".into()))?;
        if key.0 != tenant || key.1 != database {
            return Err((
                StatusCode::NOT_FOUND,
                "collection tenant/db mismatch".into(),
            ));
        }
        Ok(key)
    }
}

#[derive(Deserialize)]
struct CreateCollectionReq {
    name: String,
    #[serde(default)]
    get_or_create: bool,
    #[serde(default)]
    metadata: Option<Value>,
}

#[derive(Serialize)]
struct CollectionResp {
    id: String,
    name: String,
    tenant: String,
    database: String,
    metadata: Value,
}

#[derive(Deserialize)]
struct UpsertReq {
    ids: Vec<String>,
    embeddings: Vec<Vec<f32>>,
    #[serde(default)]
    metadatas: Option<Vec<Option<Value>>>,
}

#[derive(Deserialize)]
struct DeleteReq {
    ids: Vec<String>,
}

#[derive(Deserialize)]
struct QueryReq {
    query_embeddings: Vec<Vec<f32>>,
    n_results: usize,
    #[serde(rename = "where", default)]
    where_clause: Option<Value>,
    #[serde(default)]
    include: Option<Vec<String>>,
}

#[derive(Serialize)]
struct QueryResp {
    ids: Vec<Vec<String>>,
    distances: Vec<Vec<f32>>,
    metadatas: Vec<Vec<Value>>,
}

async fn heartbeat(
    State(state): State<Arc<MockChromaState>>,
    headers: HeaderMap,
) -> Result<Json<Value>, (StatusCode, String)> {
    state.check_auth_and_faults(&headers)?;
    Ok(Json(json!({ "nanosecond heartbeat": 1_000_000 })))
}

async fn create_collection(
    State(state): State<Arc<MockChromaState>>,
    Path((tenant, database)): Path<(String, String)>,
    headers: HeaderMap,
    Json(req): Json<CreateCollectionReq>,
) -> Result<Json<CollectionResp>, (StatusCode, String)> {
    state.check_auth_and_faults(&headers)?;
    state.create_collection_calls.fetch_add(1, Ordering::SeqCst);

    let key = (tenant.clone(), database.clone(), req.name.clone());
    let mut cols = state.collections.lock().unwrap();
    if let Some(existing) = cols.get(&key) {
        if req.get_or_create {
            return Ok(Json(CollectionResp {
                id: existing.id.clone(),
                name: existing.name.clone(),
                tenant,
                database,
                metadata: json!({ "hnsw:space": existing.space }),
            }));
        }
        return Err((StatusCode::CONFLICT, "collection already exists".into()));
    }

    let meta = req
        .metadata
        .unwrap_or_else(|| json!({ "hnsw:space": "l2" }));
    let space = meta
        .get("hnsw:space")
        .and_then(|v| v.as_str())
        .unwrap_or("l2")
        .to_string();

    let id = Uuid::new_v4().to_string();
    let stored = StoredCollection {
        id: id.clone(),
        name: req.name.clone(),
        space,
        metadata: meta.clone(),
        records: HashMap::new(),
    };
    cols.insert(key.clone(), stored);
    state.id_index.lock().unwrap().insert(id.clone(), key);

    Ok(Json(CollectionResp {
        id,
        name: req.name,
        tenant,
        database,
        metadata: meta,
    }))
}

async fn get_collection(
    State(state): State<Arc<MockChromaState>>,
    Path((tenant, database, name)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> Result<Json<CollectionResp>, (StatusCode, String)> {
    state.check_auth_and_faults(&headers)?;
    let cols = state.collections.lock().unwrap();
    let col = cols
        .get(&(tenant.clone(), database.clone(), name))
        .ok_or_else(|| (StatusCode::NOT_FOUND, "collection not found".into()))?;
    Ok(Json(CollectionResp {
        id: col.id.clone(),
        name: col.name.clone(),
        tenant,
        database,
        metadata: col.metadata.clone(),
    }))
}

async fn upsert(
    State(state): State<Arc<MockChromaState>>,
    Path((tenant, database, id)): Path<(String, String, String)>,
    headers: HeaderMap,
    Json(req): Json<UpsertReq>,
) -> Result<Json<Value>, (StatusCode, String)> {
    state.check_auth_and_faults(&headers)?;
    if req.ids.len() != req.embeddings.len() {
        return Err((
            StatusCode::BAD_REQUEST,
            "ids and embeddings length mismatch".into(),
        ));
    }

    let key = state.resolve_collection_key(&tenant, &database, &id)?;

    let mut cols = state.collections.lock().unwrap();
    let col = cols
        .get_mut(&key)
        .ok_or_else(|| (StatusCode::NOT_FOUND, "collection not found".into()))?;

    let metadatas = req.metadatas.unwrap_or_default();
    for (idx, (rec_id, emb)) in req.ids.into_iter().zip(req.embeddings).enumerate() {
        let meta = metadatas
            .get(idx)
            .and_then(|m| m.clone())
            .unwrap_or_else(|| json!({}));
        col.records.insert(rec_id, (emb, meta));
    }

    Ok(Json(json!({})))
}

async fn delete_records(
    State(state): State<Arc<MockChromaState>>,
    Path((tenant, database, id)): Path<(String, String, String)>,
    headers: HeaderMap,
    Json(req): Json<DeleteReq>,
) -> Result<Json<Value>, (StatusCode, String)> {
    state.check_auth_and_faults(&headers)?;
    let key = state.resolve_collection_key(&tenant, &database, &id)?;

    let mut cols = state.collections.lock().unwrap();
    let col = cols
        .get_mut(&key)
        .ok_or_else(|| (StatusCode::NOT_FOUND, "collection not found".into()))?;
    for rec_id in req.ids {
        col.records.remove(&rec_id);
    }

    Ok(Json(json!({})))
}

async fn query(
    State(state): State<Arc<MockChromaState>>,
    Path((tenant, database, id)): Path<(String, String, String)>,
    headers: HeaderMap,
    Json(req): Json<QueryReq>,
) -> Result<Json<QueryResp>, (StatusCode, String)> {
    state.check_auth_and_faults(&headers)?;
    let _ = req.include;
    let query_vec = req
        .query_embeddings
        .first()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing query_embeddings".into()))?;

    let key = state.resolve_collection_key(&tenant, &database, &id)?;

    let cols = state.collections.lock().unwrap();
    let col = cols
        .get(&key)
        .ok_or_else(|| (StatusCode::NOT_FOUND, "collection not found".into()))?;

    let mut hits: Vec<(String, f32, Value)> = col
        .records
        .iter()
        .filter(|(_, (_, meta))| match &req.where_clause {
            Some(w) => eval_chroma_where(w, meta),
            None => true,
        })
        .map(|(rec_id, (vec, meta))| {
            let dist = compute_distance(&col.space, query_vec, vec);
            (rec_id.clone(), dist, meta.clone())
        })
        .collect();

    hits.sort_by(|a, b| a.1.total_cmp(&b.1));
    hits.truncate(req.n_results);

    let mut ids = Vec::with_capacity(hits.len());
    let mut distances = Vec::with_capacity(hits.len());
    let mut metadatas = Vec::with_capacity(hits.len());
    for (rec_id, dist, meta) in hits {
        ids.push(rec_id);
        distances.push(dist);
        metadatas.push(meta);
    }

    Ok(Json(QueryResp {
        ids: vec![ids],
        distances: vec![distances],
        metadatas: vec![metadatas],
    }))
}

fn compute_distance(space: &str, a: &[f32], b: &[f32]) -> f32 {
    match space {
        "cosine" => {
            let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
            let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
            let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
            if na == 0.0 || nb == 0.0 {
                1.0
            } else {
                1.0 - (dot / (na * nb))
            }
        }
        "ip" => {
            let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
            1.0 - dot
        }
        _ => a.iter().zip(b).map(|(x, y)| (x - y).powi(2)).sum(),
    }
}

fn eval_chroma_where(clause: &Value, metadata: &Value) -> bool {
    let Some(obj) = clause.as_object() else {
        return false;
    };
    if let Some(arr) = obj.get("$and").and_then(|v| v.as_array()) {
        return arr.iter().all(|c| eval_chroma_where(c, metadata));
    }
    if let Some(arr) = obj.get("$or").and_then(|v| v.as_array()) {
        return arr.iter().any(|c| eval_chroma_where(c, metadata));
    }
    obj.iter().all(|(field, cond)| {
        let Some(meta_val) = metadata.get(field) else {
            return false;
        };
        let Some(ops) = cond.as_object() else {
            return meta_val == cond;
        };
        ops.iter().all(|(op, target)| match op.as_str() {
            "$eq" => json_scalar_eq(meta_val, target),
            "$in" => target
                .as_array()
                .is_some_and(|arr| arr.iter().any(|item| json_scalar_eq(meta_val, item))),
            "$gte" => match (meta_val.as_f64(), target.as_f64()) {
                (Some(a), Some(b)) => a >= b,
                _ => false,
            },
            "$lte" => match (meta_val.as_f64(), target.as_f64()) {
                (Some(a), Some(b)) => a <= b,
                _ => false,
            },
            _ => false,
        })
    })
}

fn json_scalar_eq(a: &Value, b: &Value) -> bool {
    match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) => (x - y).abs() < f64::EPSILON,
        _ => a == b,
    }
}

async fn spawn_mock_chroma(state: Arc<MockChromaState>) -> String {
    let app = Router::new()
        .route("/api/v2/heartbeat", get(heartbeat))
        .route(
            "/api/v2/tenants/{tenant}/databases/{database}/collections",
            post(create_collection),
        )
        .route(
            "/api/v2/tenants/{tenant}/databases/{database}/collections/{name}",
            get(get_collection),
        )
        .route(
            "/api/v2/tenants/{tenant}/databases/{database}/collections/{id}/upsert",
            post(upsert),
        )
        .route(
            "/api/v2/tenants/{tenant}/databases/{database}/collections/{id}/delete",
            post(delete_records),
        )
        .route(
            "/api/v2/tenants/{tenant}/databases/{database}/collections/{id}/query",
            post(query),
        )
        .with_state(state);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

fn test_config(base_url: String, auth_token: Option<SecretString>) -> ChromaConfig {
    ChromaConfig {
        base_url,
        tenant: "default_tenant".into(),
        database: "default_database".into(),
        auth_token,
        collection_prefix: "aether".into(),
    }
}

#[tokio::test]
async fn chroma_contract_vector_store() {
    let state = Arc::new(MockChromaState::new(None));
    let base_url = spawn_mock_chroma(state).await;
    let store = ChromaVectorStore::new(test_config(base_url.clone(), None)).unwrap();

    assert_eq!(store.name(), "chroma");
    assert!(store.capabilities().supports_filter_expr);
    assert_eq!(
        store.health_check().await.unwrap(),
        ConnectorHealth::Healthy
    );

    contract::vector_store(&store).await;
    store.close().await.unwrap();
}

#[tokio::test]
async fn chroma_http_status_error_mapping() {
    let state = Arc::new(MockChromaState::new(None));
    let base_url = spawn_mock_chroma(state.clone()).await;
    let store = ChromaVectorStore::new(test_config(base_url, None)).unwrap();
    let ns = ProjectId::new(format!("err-{}", Uuid::new_v4())).unwrap();
    store
        .initialize(&ns, 2, DistanceMetric::Cosine)
        .await
        .unwrap();

    state.push_forced_error(StatusCode::TOO_MANY_REQUESTS, "rate limited");
    let err_429 = store.search(&ns, &[1.0, 0.0], 5, None).await.unwrap_err();
    assert!(
        matches!(
            err_429,
            AetherError::Transient {
                connector: "chroma",
                ..
            }
        ),
        "expected Transient for 429, got {err_429:?}"
    );
    assert!(err_429.is_retryable());

    state.push_forced_error(StatusCode::SERVICE_UNAVAILABLE, "backend unavailable");
    let err_503 = store.search(&ns, &[1.0, 0.0], 5, None).await.unwrap_err();
    assert!(
        matches!(
            err_503,
            AetherError::Transient {
                connector: "chroma",
                ..
            }
        ),
        "expected Transient for 503, got {err_503:?}"
    );
    assert!(err_503.is_retryable());

    state.push_forced_error(StatusCode::UNAUTHORIZED, "invalid token");
    let err_401 = store.search(&ns, &[1.0, 0.0], 5, None).await.unwrap_err();
    assert!(
        matches!(
            err_401,
            AetherError::Permanent {
                connector: "chroma",
                ..
            }
        ),
        "expected Permanent for 401, got {err_401:?}"
    );
    assert!(!err_401.is_retryable());

    state.push_forced_error(StatusCode::BAD_REQUEST, "malformed query");
    let err_400 = store.search(&ns, &[1.0, 0.0], 5, None).await.unwrap_err();
    assert!(
        matches!(
            err_400,
            AetherError::Permanent {
                connector: "chroma",
                ..
            }
        ),
        "expected Permanent for 400, got {err_400:?}"
    );
    assert!(!err_400.is_retryable());
}

#[tokio::test]
async fn chroma_auth_token_and_collection_caching() {
    let state = Arc::new(MockChromaState::new(Some("chroma-secret".into())));
    let base_url = spawn_mock_chroma(state.clone()).await;

    let unauth = ChromaVectorStore::new(test_config(base_url.clone(), None)).unwrap();
    let ns = ProjectId::new(format!("auth-{}", Uuid::new_v4())).unwrap();
    let err = unauth
        .initialize(&ns, 2, DistanceMetric::Cosine)
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        AetherError::Permanent {
            connector: "chroma",
            ..
        }
    ));

    let authed = ChromaVectorStore::new(test_config(
        base_url,
        Some(SecretString::new("chroma-secret")),
    ))
    .unwrap();
    authed
        .initialize(&ns, 2, DistanceMetric::Cosine)
        .await
        .unwrap();
    let id = MemoryId::new();
    authed
        .upsert_batch(
            &ns,
            &[VectorRecord {
                id,
                vector: vec![1.0, 0.0],
                metadata: json!({"kind": "fact"}),
            }],
        )
        .await
        .unwrap();
    let hits = authed.search(&ns, &[1.0, 0.0], 5, None).await.unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, id);
    authed.delete(&ns, &[id]).await.unwrap();

    assert_eq!(state.create_collection_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn chroma_distance_metrics_filters_and_validation() {
    let state = Arc::new(MockChromaState::new(None));
    let base_url = spawn_mock_chroma(state).await;
    let store = ChromaVectorStore::new(test_config(base_url.clone(), None)).unwrap();

    for metric in [
        DistanceMetric::Cosine,
        DistanceMetric::Dot,
        DistanceMetric::Euclidean,
    ] {
        let ns = ProjectId::new(format!("metric-{}", Uuid::new_v4())).unwrap();
        store.initialize(&ns, 2, metric).await.unwrap();
        let near = MemoryId::new();
        let far = MemoryId::new();
        store
            .upsert_batch(
                &ns,
                &[
                    VectorRecord {
                        id: near,
                        vector: vec![1.0, 0.0],
                        metadata: json!({"tag": "a", "rank": 5}),
                    },
                    VectorRecord {
                        id: far,
                        vector: vec![0.0, 2.0],
                        metadata: json!({"tag": "b", "rank": 10}),
                    },
                ],
            )
            .await
            .unwrap();

        let hits = store.search(&ns, &[1.0, 0.0], 2, None).await.unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].id, near);
        assert!(
            hits[0].score > hits[1].score,
            "higher similarity score must mean closer for {metric:?}: {:?}",
            hits
        );

        let filter = FilterExpr::Or(vec![
            FilterExpr::Eq {
                field: "tag".into(),
                value: FilterValue::String("a".into()),
            },
            FilterExpr::Range {
                field: "rank".into(),
                gte: Some(8.0),
                lte: Some(10.0),
            },
        ]);
        let filtered = store
            .search(&ns, &[1.0, 0.0], 5, Some(&filter))
            .await
            .unwrap();
        assert_eq!(filtered.len(), 2);

        let bounded = FilterExpr::Range {
            field: "rank".into(),
            gte: Some(4.0),
            lte: Some(6.0),
        };
        let bounded_hits = store
            .search(&ns, &[1.0, 0.0], 5, Some(&bounded))
            .await
            .unwrap();
        assert_eq!(bounded_hits.len(), 1);
        assert_eq!(bounded_hits[0].id, near);

        let query_dim_err = store.search(&ns, &[1.0, 0.0, 0.0], 5, None).await;
        assert!(matches!(query_dim_err, Err(AetherError::InvalidInput(_))));
    }

    let uninit = ProjectId::new(format!("uninit-{}", Uuid::new_v4())).unwrap();
    assert!(store.search(&uninit, &[1.0, 0.0], 5, None).await.is_err());

    let empty_url_err = ChromaVectorStore::new(test_config("   ".into(), None));
    assert!(matches!(empty_url_err, Err(AetherError::InvalidInput(_))));

    let ns_edge = ProjectId::new(format!("edge-{}", Uuid::new_v4())).unwrap();
    store
        .initialize(&ns_edge, 2, DistanceMetric::Cosine)
        .await
        .unwrap();
    let nan_err = store
        .upsert_batch(
            &ns_edge,
            &[VectorRecord {
                id: MemoryId::new(),
                vector: vec![f32::NAN, 1.0],
                metadata: json!({}),
            }],
        )
        .await
        .unwrap_err();
    assert!(matches!(nan_err, AetherError::InvalidInput(_)));

    let empty_or_hits = store
        .search(&ns_edge, &[1.0, 0.0], 5, Some(&FilterExpr::Or(vec![])))
        .await
        .unwrap();
    assert!(empty_or_hits.is_empty());

    let fresh = ChromaVectorStore::new(test_config(base_url, None)).unwrap();
    assert_eq!(
        fresh.describe(&ns_edge).await.unwrap(),
        Some(aether_core::IndexSpec {
            dimensions: 2,
            metric: DistanceMetric::Cosine,
        })
    );
    let missing = ProjectId::new("missing-chroma-proj").unwrap();
    assert_eq!(fresh.describe(&missing).await.unwrap(), None);
}

#[tokio::test]
#[ignore = "requires live Chroma (default http://127.0.0.1:18000)"]
async fn live_chroma_contract_vector_store() {
    let base_url =
        std::env::var("CHROMA_URL").unwrap_or_else(|_| "http://127.0.0.1:18000".to_string());
    let mut cfg = test_config(base_url, None);
    cfg.collection_prefix = format!("live_{}", Uuid::new_v4().simple());

    let store = ChromaVectorStore::new(cfg.clone()).unwrap();
    assert_eq!(
        store.health_check().await.unwrap(),
        ConnectorHealth::Healthy
    );
    contract::vector_store(&store).await;

    let ns = ProjectId::new(format!("live-desc-{}", Uuid::new_v4())).unwrap();
    store
        .initialize(&ns, 3, DistanceMetric::Euclidean)
        .await
        .unwrap();
    let fresh = ChromaVectorStore::new(cfg).unwrap();
    assert_eq!(
        fresh.describe(&ns).await.unwrap(),
        Some(aether_core::IndexSpec {
            dimensions: 3,
            metric: DistanceMetric::Euclidean,
        })
    );
}
