#![cfg(all(feature = "qdrant", feature = "contract-suite"))]

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use muninn_aether_adapters::contract;
use muninn_aether_adapters::storage::qdrant::{QdrantConfig, QdrantVectorStore};
use muninn_aether_core::{
    AetherError, ConnectorHealth, DistanceMetric, FilterExpr, FilterValue, MemoryId,
    OutboundConnector, ProjectId, SecretString, VectorRecord, VectorStore,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::net::TcpListener;

#[derive(Clone)]
struct MockPoint {
    id: MemoryId,
    vector: Vec<f32>,
    payload: Value,
}

struct MockCollection {
    size: usize,
    distance: String,
    points: HashMap<MemoryId, MockPoint>,
}

#[derive(Default)]
struct MockQdrantState {
    expected_api_key: Option<String>,
    collections: HashMap<String, MockCollection>,
    injected_errors: VecDeque<(StatusCode, String)>,
    saw_api_key_headers: Vec<Option<String>>,
    last_search_filter: Option<Value>,
}

type SharedState = Arc<Mutex<MockQdrantState>>;

#[derive(Deserialize)]
struct WaitQuery {
    wait: Option<bool>,
}

#[derive(Deserialize)]
struct CreateCollectionPayload {
    vectors: VectorConfigPayload,
}

#[derive(Deserialize, Serialize)]
struct VectorConfigPayload {
    size: usize,
    distance: String,
}

#[derive(Deserialize)]
struct UpsertPointsPayload {
    points: Vec<PointPayload>,
}

#[derive(Deserialize)]
struct PointPayload {
    id: MemoryId,
    vector: Vec<f32>,
    payload: Option<Value>,
}

#[derive(Deserialize)]
struct DeletePointsPayload {
    points: Vec<MemoryId>,
}

#[derive(Deserialize)]
struct SearchPayload {
    vector: Vec<f32>,
    limit: usize,
    with_payload: Option<bool>,
    filter: Option<Value>,
}

fn preflight(state: &SharedState, headers: &HeaderMap) -> Option<Response> {
    let mut guard = state.lock().unwrap();
    let header_val = headers
        .get("api-key")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    guard.saw_api_key_headers.push(header_val.clone());

    if let Some(expected) = &guard.expected_api_key {
        if header_val.as_deref() != Some(expected.as_str()) {
            return Some(
                (
                    StatusCode::UNAUTHORIZED,
                    Json(json!({"status": {"error": "unauthorized"}})),
                )
                    .into_response(),
            );
        }
    }

    if let Some((status, message)) = guard.injected_errors.pop_front() {
        return Some((status, Json(json!({"status": {"error": message}}))).into_response());
    }

    None
}

async fn healthz(State(state): State<SharedState>, headers: HeaderMap) -> Response {
    if let Some(err) = preflight(&state, &headers) {
        return err;
    }
    (StatusCode::OK, "healthz check passed").into_response()
}

async fn create_collection(
    State(state): State<SharedState>,
    Path(collection): Path<String>,
    headers: HeaderMap,
    Json(body): Json<CreateCollectionPayload>,
) -> Response {
    if let Some(err) = preflight(&state, &headers) {
        return err;
    }

    if body.vectors.size == 0
        || !matches!(body.vectors.distance.as_str(), "Cosine" | "Dot" | "Euclid")
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"status": {"error": "invalid vector config"}})),
        )
            .into_response();
    }

    let mut guard = state.lock().unwrap();
    if guard.collections.contains_key(&collection) {
        return (
            StatusCode::CONFLICT,
            Json(json!({"status": {"error": "collection already exists"}})),
        )
            .into_response();
    }

    guard.collections.insert(
        collection,
        MockCollection {
            size: body.vectors.size,
            distance: body.vectors.distance,
            points: HashMap::new(),
        },
    );
    (
        StatusCode::OK,
        Json(json!({"result": true, "status": "ok"})),
    )
        .into_response()
}

async fn get_collection(
    State(state): State<SharedState>,
    Path(collection): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Some(err) = preflight(&state, &headers) {
        return err;
    }

    let guard = state.lock().unwrap();
    let Some(col) = guard.collections.get(&collection) else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"status": {"error": "collection not found"}})),
        )
            .into_response();
    };

    (
        StatusCode::OK,
        Json(json!({
            "result": {
                "config": {
                    "params": {
                        "vectors": {
                            "size": col.size,
                            "distance": col.distance
                        }
                    }
                }
            },
            "status": "ok"
        })),
    )
        .into_response()
}

async fn upsert_points(
    State(state): State<SharedState>,
    Path(collection): Path<String>,
    Query(query): Query<WaitQuery>,
    headers: HeaderMap,
    Json(body): Json<UpsertPointsPayload>,
) -> Response {
    if let Some(err) = preflight(&state, &headers) {
        return err;
    }
    if query.wait != Some(true) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"status": {"error": "expected wait=true"}})),
        )
            .into_response();
    }

    let mut guard = state.lock().unwrap();
    let Some(col) = guard.collections.get_mut(&collection) else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"status": {"error": "collection not found"}})),
        )
            .into_response();
    };

    for pt in body.points {
        if pt.vector.len() != col.size {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"status": {"error": "vector dimension mismatch"}})),
            )
                .into_response();
        }
        col.points.insert(
            pt.id,
            MockPoint {
                id: pt.id,
                vector: pt.vector,
                payload: pt.payload.unwrap_or_else(|| json!({})),
            },
        );
    }

    (
        StatusCode::OK,
        Json(json!({"result": {"status": "completed"}, "status": "ok"})),
    )
        .into_response()
}

async fn delete_points(
    State(state): State<SharedState>,
    Path(collection): Path<String>,
    Query(query): Query<WaitQuery>,
    headers: HeaderMap,
    Json(body): Json<DeletePointsPayload>,
) -> Response {
    if let Some(err) = preflight(&state, &headers) {
        return err;
    }
    if query.wait != Some(true) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"status": {"error": "expected wait=true"}})),
        )
            .into_response();
    }

    let mut guard = state.lock().unwrap();
    let Some(col) = guard.collections.get_mut(&collection) else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"status": {"error": "collection not found"}})),
        )
            .into_response();
    };

    for id in body.points {
        col.points.remove(&id);
    }

    (
        StatusCode::OK,
        Json(json!({"result": {"status": "completed"}, "status": "ok"})),
    )
        .into_response()
}

async fn search_points(
    State(state): State<SharedState>,
    Path(collection): Path<String>,
    headers: HeaderMap,
    Json(body): Json<SearchPayload>,
) -> Response {
    if let Some(err) = preflight(&state, &headers) {
        return err;
    }

    let mut guard = state.lock().unwrap();
    guard.last_search_filter = body.filter.clone();

    let Some(col) = guard.collections.get(&collection) else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"status": {"error": "collection not found"}})),
        )
            .into_response();
    };

    if body.vector.len() != col.size {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"status": {"error": "query vector dimension mismatch"}})),
        )
            .into_response();
    }

    let metric = match col.distance.as_str() {
        "Cosine" => DistanceMetric::Cosine,
        "Dot" => DistanceMetric::Dot,
        "Euclid" => DistanceMetric::Euclidean,
        _ => DistanceMetric::Cosine,
    };

    let with_payload = body.with_payload.unwrap_or(false);
    let mut hits: Vec<(f32, &MockPoint)> = col
        .points
        .values()
        .filter(|pt| match &body.filter {
            Some(f) => eval_qdrant_filter(f, &pt.payload),
            None => true,
        })
        .map(|pt| {
            let raw = match metric {
                DistanceMetric::Euclidean => {
                    let sq: f32 = body
                        .vector
                        .iter()
                        .zip(&pt.vector)
                        .map(|(a, b)| (a - b).powi(2))
                        .sum();
                    sq.sqrt()
                }
                _ => metric.score(&body.vector, &pt.vector),
            };
            (raw, pt)
        })
        .collect();

    if metric == DistanceMetric::Euclidean {
        hits.sort_by(|a, b| a.0.total_cmp(&b.0));
    } else {
        hits.sort_by(|a, b| b.0.total_cmp(&a.0));
    }
    hits.truncate(body.limit);

    let result: Vec<Value> = hits
        .into_iter()
        .map(|(score, pt)| {
            if with_payload {
                json!({
                    "id": pt.id,
                    "version": 0,
                    "score": score,
                    "payload": pt.payload
                })
            } else {
                json!({
                    "id": pt.id,
                    "version": 0,
                    "score": score
                })
            }
        })
        .collect();

    (
        StatusCode::OK,
        Json(json!({"result": result, "status": "ok"})),
    )
        .into_response()
}

fn eval_qdrant_filter(filter: &Value, payload: &Value) -> bool {
    if let Some(has_id) = filter.get("has_id").and_then(Value::as_array) {
        if has_id.is_empty() {
            return false;
        }
    }

    if let Some(key) = filter.get("key").and_then(Value::as_str) {
        let field_val = payload.get(key);
        if let Some(matcher) = filter.get("match") {
            if let Some(target) = matcher.get("value") {
                return field_val.is_some_and(|v| v == target);
            }
            if let Some(any) = matcher.get("any").and_then(Value::as_array) {
                return field_val.is_some_and(|v| any.iter().any(|candidate| candidate == v));
            }
        }
        if let Some(range) = filter.get("range") {
            let Some(num) = field_val.and_then(Value::as_f64) else {
                return false;
            };
            let gte_ok = range
                .get("gte")
                .and_then(Value::as_f64)
                .is_none_or(|min| num >= min);
            let lte_ok = range
                .get("lte")
                .and_then(Value::as_f64)
                .is_none_or(|max| num <= max);
            return gte_ok && lte_ok;
        }
        return false;
    }

    let must_ok = match filter.get("must").and_then(Value::as_array) {
        Some(conds) => conds.iter().all(|c| eval_qdrant_filter(c, payload)),
        None => true,
    };
    let should_ok = match filter.get("should").and_then(Value::as_array) {
        Some(conds) if !conds.is_empty() => conds.iter().any(|c| eval_qdrant_filter(c, payload)),
        _ => true,
    };
    must_ok && should_ok
}

async fn spawn_mock_qdrant(expected_api_key: Option<&str>) -> (String, SharedState) {
    let state = Arc::new(Mutex::new(MockQdrantState {
        expected_api_key: expected_api_key.map(str::to_string),
        ..Default::default()
    }));

    let app = Router::new()
        .route("/healthz", get(healthz))
        .route(
            "/collections/{collection}",
            put(create_collection).get(get_collection),
        )
        .route("/collections/{collection}/points", put(upsert_points))
        .route(
            "/collections/{collection}/points/delete",
            post(delete_points),
        )
        .route(
            "/collections/{collection}/points/search",
            post(search_points),
        )
        .with_state(state.clone());

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    (format!("http://{addr}"), state)
}

#[tokio::test]
async fn qdrant_contract_vector_store() {
    let (base_url, _state) = spawn_mock_qdrant(None).await;
    let store = QdrantVectorStore::new(QdrantConfig {
        base_url,
        api_key: None,
        collection_prefix: "contract".into(),
    })
    .unwrap();

    contract::vector_store(&store).await;
}

#[tokio::test]
async fn qdrant_initialize_idempotent_and_rejects_spec_mismatch() {
    let (base_url, _state) = spawn_mock_qdrant(None).await;
    let store_a = QdrantVectorStore::new(QdrantConfig {
        base_url: base_url.clone(),
        api_key: None,
        collection_prefix: "idem".into(),
    })
    .unwrap();
    let ns = ProjectId::new("proj-idem").unwrap();

    store_a
        .initialize(&ns, 3, DistanceMetric::Cosine)
        .await
        .unwrap();
    store_a
        .initialize(&ns, 3, DistanceMetric::Cosine)
        .await
        .unwrap();

    let mismatch_same_client = store_a
        .initialize(&ns, 4, DistanceMetric::Cosine)
        .await
        .unwrap_err();
    assert!(matches!(mismatch_same_client, AetherError::InvalidInput(_)));

    let store_b = QdrantVectorStore::new(QdrantConfig {
        base_url,
        api_key: None,
        collection_prefix: "idem".into(),
    })
    .unwrap();
    store_b
        .initialize(&ns, 3, DistanceMetric::Cosine)
        .await
        .unwrap();
    let mismatch_fresh_client = store_b
        .initialize(&ns, 3, DistanceMetric::Dot)
        .await
        .unwrap_err();
    assert!(matches!(
        mismatch_fresh_client,
        AetherError::InvalidInput(_)
    ));
}

#[tokio::test]
async fn qdrant_filter_translation_covers_eq_in_range_and_or() {
    let (base_url, state) = spawn_mock_qdrant(None).await;
    let store = QdrantVectorStore::new(QdrantConfig {
        base_url,
        api_key: None,
        collection_prefix: "flt".into(),
    })
    .unwrap();
    let ns = ProjectId::new("proj-flt").unwrap();
    store.initialize(&ns, 2, DistanceMetric::Dot).await.unwrap();

    let id1 = MemoryId::new();
    let id2 = MemoryId::new();
    let id3 = MemoryId::new();
    store
        .upsert_batch(
            &ns,
            &[
                VectorRecord {
                    id: id1,
                    vector: vec![1.0, 0.0],
                    metadata: json!({"tier": "semantic", "pinned": true, "rank": 1, "score": 0.9}),
                },
                VectorRecord {
                    id: id2,
                    vector: vec![0.9, 0.1],
                    metadata: json!({"tier": "episodic", "pinned": false, "rank": 2, "score": 0.6}),
                },
                VectorRecord {
                    id: id3,
                    vector: vec![0.8, 0.2],
                    metadata: json!({"tier": "procedural", "pinned": true, "rank": 3, "score": 0.2}),
                },
            ],
        )
        .await
        .unwrap();

    let expr = FilterExpr::Or(vec![
        FilterExpr::And(vec![
            FilterExpr::Eq {
                field: "pinned".into(),
                value: FilterValue::Bool(true),
            },
            FilterExpr::Range {
                field: "score".into(),
                gte: Some(0.5),
                lte: Some(1.0),
            },
        ]),
        FilterExpr::In {
            field: "rank".into(),
            values: vec![FilterValue::Int(2)],
        },
    ]);

    let hits = store
        .search(&ns, &[1.0, 0.0], 10, Some(&expr))
        .await
        .unwrap();
    let hit_ids: Vec<MemoryId> = hits.iter().map(|h| h.id).collect();
    assert_eq!(hit_ids, vec![id1, id2]);

    let sent_filter = state.lock().unwrap().last_search_filter.clone().unwrap();
    assert!(sent_filter.get("should").is_some());

    let eq_hits = store
        .search(
            &ns,
            &[1.0, 0.0],
            10,
            Some(&FilterExpr::Eq {
                field: "tier".into(),
                value: FilterValue::String("procedural".into()),
            }),
        )
        .await
        .unwrap();
    assert_eq!(eq_hits.len(), 1);
    assert_eq!(eq_hits[0].id, id3);

    let empty_or_hits = store
        .search(&ns, &[1.0, 0.0], 10, Some(&FilterExpr::Or(vec![])))
        .await
        .unwrap();
    assert!(empty_or_hits.is_empty());

    let nan_range = store
        .search(
            &ns,
            &[1.0, 0.0],
            10,
            Some(&FilterExpr::Range {
                field: "score".into(),
                gte: Some(f64::NAN),
                lte: None,
            }),
        )
        .await
        .unwrap_err();
    assert!(matches!(nan_range, AetherError::InvalidInput(_)));

    let query_dim_err = store
        .search(&ns, &[1.0, 0.0, 0.0], 5, None)
        .await
        .unwrap_err();
    assert!(matches!(query_dim_err, AetherError::InvalidInput(_)));
}

#[tokio::test]
async fn qdrant_sends_api_key_and_maps_http_status_errors() {
    let (base_url, state) = spawn_mock_qdrant(Some("secret-qdrant-token")).await;

    let unauth_store = QdrantVectorStore::new(QdrantConfig {
        base_url: base_url.clone(),
        api_key: None,
        collection_prefix: "auth".into(),
    })
    .unwrap();
    let ns = ProjectId::new("proj-err").unwrap();
    let err_401 = unauth_store
        .initialize(&ns, 2, DistanceMetric::Cosine)
        .await
        .unwrap_err();
    assert!(matches!(
        err_401,
        AetherError::Permanent {
            connector: "qdrant",
            ..
        }
    ));
    assert!(!err_401.is_retryable());

    let auth_store = QdrantVectorStore::new(QdrantConfig {
        base_url,
        api_key: Some(SecretString::new("secret-qdrant-token")),
        collection_prefix: "auth".into(),
    })
    .unwrap();
    auth_store
        .initialize(&ns, 2, DistanceMetric::Euclidean)
        .await
        .unwrap();

    assert!(state
        .lock()
        .unwrap()
        .saw_api_key_headers
        .iter()
        .any(|h| h.as_deref() == Some("secret-qdrant-token")));

    {
        let mut guard = state.lock().unwrap();
        guard
            .injected_errors
            .push_back((StatusCode::TOO_MANY_REQUESTS, "rate limited".into()));
        guard
            .injected_errors
            .push_back((StatusCode::SERVICE_UNAVAILABLE, "overloaded".into()));
        guard
            .injected_errors
            .push_back((StatusCode::BAD_REQUEST, "bad filter".into()));
    }

    let err_429 = auth_store
        .search(&ns, &[1.0, 0.0], 5, None)
        .await
        .unwrap_err();
    assert!(matches!(
        err_429,
        AetherError::Transient {
            connector: "qdrant",
            ..
        }
    ));
    assert!(err_429.is_retryable());

    let err_503 = auth_store
        .delete(&ns, &[MemoryId::new()])
        .await
        .unwrap_err();
    assert!(matches!(
        err_503,
        AetherError::Transient {
            connector: "qdrant",
            ..
        }
    ));
    assert!(err_503.is_retryable());

    let err_400 = auth_store
        .upsert_batch(
            &ns,
            &[VectorRecord {
                id: MemoryId::new(),
                vector: vec![1.0, 0.0],
                metadata: json!({}),
            }],
        )
        .await
        .unwrap_err();
    assert!(matches!(
        err_400,
        AetherError::Permanent {
            connector: "qdrant",
            ..
        }
    ));
    assert!(!err_400.is_retryable());
}

#[tokio::test]
async fn qdrant_connector_metadata_health_and_input_validation() {
    let (base_url, state) = spawn_mock_qdrant(None).await;
    let store = QdrantVectorStore::new(QdrantConfig {
        base_url,
        api_key: None,
        collection_prefix: "meta".into(),
    })
    .unwrap();

    assert_eq!(store.name(), "qdrant");
    assert!(store.capabilities().supports_filter_expr);
    assert_eq!(
        store.health_check().await.unwrap(),
        ConnectorHealth::Healthy
    );

    let ns = ProjectId::new("proj-euclid").unwrap();
    store
        .initialize(&ns, 2, DistanceMetric::Euclidean)
        .await
        .unwrap();
    assert_eq!(
        state
            .lock()
            .unwrap()
            .collections
            .values()
            .next()
            .unwrap()
            .distance,
        "Euclid"
    );

    let near = MemoryId::new();
    let far = MemoryId::new();
    store
        .upsert_batch(
            &ns,
            &[
                VectorRecord {
                    id: near,
                    vector: vec![1.0, 0.0],
                    metadata: json!({"k": "near"}),
                },
                VectorRecord {
                    id: far,
                    vector: vec![10.0, 0.0],
                    metadata: json!({"k": "far"}),
                },
            ],
        )
        .await
        .unwrap();
    let euclid_hits = store.search(&ns, &[1.0, 0.0], 2, None).await.unwrap();
    assert_eq!(euclid_hits.len(), 2);
    assert_eq!(euclid_hits[0].id, near);
    assert!(
        euclid_hits[0].score > euclid_hits[1].score,
        "Euclidean closer hit must have higher score: {euclid_hits:?}"
    );
    assert!(euclid_hits[1].score <= 0.0);

    // Distinct project IDs with punctuation must not collide onto the same collection.
    let ns_slash = ProjectId::new("org/a").unwrap();
    let ns_colon = ProjectId::new("org:a").unwrap();
    store
        .initialize(&ns_slash, 2, DistanceMetric::Cosine)
        .await
        .unwrap();
    store
        .initialize(&ns_colon, 2, DistanceMetric::Cosine)
        .await
        .unwrap();
    let slash_id = MemoryId::new();
    store
        .upsert_batch(
            &ns_slash,
            &[VectorRecord {
                id: slash_id,
                vector: vec![1.0, 0.0],
                metadata: json!({}),
            }],
        )
        .await
        .unwrap();
    assert!(store
        .search(&ns_colon, &[1.0, 0.0], 5, None)
        .await
        .unwrap()
        .is_empty());

    let nan_err = store
        .upsert_batch(
            &ns,
            &[VectorRecord {
                id: MemoryId::new(),
                vector: vec![f32::NAN, 0.0],
                metadata: json!({}),
            }],
        )
        .await
        .unwrap_err();
    assert!(matches!(nan_err, AetherError::InvalidInput(_)));

    let zero_dim = store
        .initialize(&ns, 0, DistanceMetric::Cosine)
        .await
        .unwrap_err();
    assert!(matches!(zero_dim, AetherError::InvalidInput(_)));

    let empty_url = QdrantVectorStore::new(QdrantConfig {
        base_url: "   ".into(),
        api_key: None,
        collection_prefix: "x".into(),
    })
    .unwrap_err();
    assert!(matches!(empty_url, AetherError::InvalidInput(_)));

    store.close().await.unwrap();
}

#[tokio::test]
async fn qdrant_describe_resolves_across_fresh_clients() {
    let (base_url, _state) = spawn_mock_qdrant(None).await;
    let ns = ProjectId::new("proj-desc").unwrap();
    let missing = ProjectId::new("proj-missing").unwrap();

    let store_a = QdrantVectorStore::new(QdrantConfig {
        base_url: base_url.clone(),
        api_key: None,
        collection_prefix: "desc".into(),
    })
    .unwrap();
    store_a
        .initialize(&ns, 3, DistanceMetric::Euclidean)
        .await
        .unwrap();

    let store_b = QdrantVectorStore::new(QdrantConfig {
        base_url,
        api_key: None,
        collection_prefix: "desc".into(),
    })
    .unwrap();
    let spec = store_b.describe(&ns).await.unwrap();
    assert_eq!(
        spec,
        Some(muninn_aether_core::IndexSpec {
            dimensions: 3,
            metric: DistanceMetric::Euclidean,
        })
    );
    assert_eq!(store_b.describe(&missing).await.unwrap(), None);
}

#[tokio::test]
#[ignore = "requires live Qdrant (default http://127.0.0.1:16333)"]
async fn live_qdrant_contract_vector_store() {
    let base_url =
        std::env::var("QDRANT_URL").unwrap_or_else(|_| "http://127.0.0.1:16333".to_string());
    let prefix = format!("live-{}", uuid::Uuid::new_v4().simple());
    let store = QdrantVectorStore::new(QdrantConfig {
        base_url: base_url.clone(),
        api_key: None,
        collection_prefix: prefix.clone(),
    })
    .unwrap();

    assert_eq!(
        store.health_check().await.unwrap(),
        ConnectorHealth::Healthy
    );
    contract::vector_store(&store).await;

    let ns = ProjectId::new(format!("live-desc-{}", uuid::Uuid::new_v4())).unwrap();
    store
        .initialize(&ns, 3, DistanceMetric::Euclidean)
        .await
        .unwrap();
    let fresh = QdrantVectorStore::new(QdrantConfig {
        base_url,
        api_key: None,
        collection_prefix: prefix,
    })
    .unwrap();
    assert_eq!(
        fresh.describe(&ns).await.unwrap(),
        Some(muninn_aether_core::IndexSpec {
            dimensions: 3,
            metric: DistanceMetric::Euclidean,
        })
    );
}
