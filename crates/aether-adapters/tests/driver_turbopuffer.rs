#![cfg(all(feature = "turbopuffer", feature = "contract-suite"))]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use nytheris_aether_adapters::contract;
use nytheris_aether_adapters::storage::turbopuffer::{TurbopufferConfig, TurbopufferVectorStore};
use nytheris_aether_core::{
    AetherError, ConnectorHealth, DistanceMetric, FilterExpr, FilterValue, MemoryId,
    OutboundConnector, ProjectId, SecretString, VectorRecord, VectorStore,
};
use serde_json::{json, Map, Value};
use tokio::net::TcpListener;

#[derive(Clone)]
struct MockRow {
    id: String,
    vector: Vec<f32>,
    attributes: Value,
}

#[derive(Default)]
struct MockNamespace {
    metric: String,
    rows: HashMap<String, MockRow>,
}

struct CapturedRequest {
    path: String,
    body: Value,
}

#[derive(Default)]
struct MockState {
    expected_token: String,
    forced_error: Option<(StatusCode, String)>,
    namespaces: HashMap<String, MockNamespace>,
    requests: Vec<CapturedRequest>,
}

type SharedState = Arc<Mutex<MockState>>;

fn check_auth_and_forced(
    state: &mut MockState,
    headers: &HeaderMap,
) -> std::result::Result<(), (StatusCode, String)> {
    if let Some((status, msg)) = state.forced_error.take() {
        return Err((status, msg));
    }
    let expected = format!("Bearer {}", state.expected_token);
    let actual = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    if actual != Some(expected.as_str()) {
        return Err((StatusCode::UNAUTHORIZED, "unauthorized".into()));
    }
    Ok(())
}

async fn handle_list_namespaces(
    State(state): State<SharedState>,
    headers: HeaderMap,
) -> std::result::Result<Json<Value>, (StatusCode, String)> {
    let mut guard = state.lock().unwrap();
    check_auth_and_forced(&mut guard, &headers)?;
    let names: Vec<Value> = guard
        .namespaces
        .keys()
        .map(|k| json!({ "id": k }))
        .collect();
    Ok(Json(json!({ "namespaces": names })))
}

async fn handle_write_namespace(
    State(state): State<SharedState>,
    Path(namespace): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> std::result::Result<Json<Value>, (StatusCode, String)> {
    let mut guard = state.lock().unwrap();
    check_auth_and_forced(&mut guard, &headers)?;
    guard.requests.push(CapturedRequest {
        path: format!("/v1/namespaces/{namespace}"),
        body: body.clone(),
    });

    let ns = guard.namespaces.entry(namespace).or_default();
    if let Some(metric) = body.get("distance_metric").and_then(|v| v.as_str()) {
        ns.metric = metric.to_string();
    } else if ns.metric.is_empty() {
        ns.metric = "cosine_distance".to_string();
    }

    if let Some(rows) = body.get("upsert_rows").and_then(|v| v.as_array()) {
        for row in rows {
            let obj = row
                .as_object()
                .ok_or((StatusCode::BAD_REQUEST, "row must be an object".into()))?;
            let id = obj
                .get("id")
                .and_then(|v| v.as_str())
                .ok_or((StatusCode::BAD_REQUEST, "row missing id".into()))?
                .to_string();
            let vec_vals = obj
                .get("vector")
                .and_then(|v| v.as_array())
                .ok_or((StatusCode::BAD_REQUEST, "row missing vector".into()))?;
            let vector: Vec<f32> = vec_vals
                .iter()
                .map(|v| {
                    v.as_f64()
                        .map(|f| f as f32)
                        .ok_or((StatusCode::BAD_REQUEST, "vector element not numeric".into()))
                })
                .collect::<std::result::Result<_, _>>()?;

            let mut attrs = Map::new();
            for (k, v) in obj {
                if k != "id" && k != "vector" {
                    attrs.insert(k.clone(), v.clone());
                }
            }
            ns.rows.insert(
                id.clone(),
                MockRow {
                    id,
                    vector,
                    attributes: Value::Object(attrs),
                },
            );
        }
    }

    if let Some(deletes) = body.get("deletes").and_then(|v| v.as_array()) {
        for id_val in deletes {
            if let Some(id) = id_val.as_str() {
                ns.rows.remove(id);
            }
        }
    }

    Ok(Json(json!({ "status": "OK" })))
}

async fn handle_query_namespace(
    State(state): State<SharedState>,
    Path(namespace): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> std::result::Result<Json<Value>, (StatusCode, String)> {
    let mut guard = state.lock().unwrap();
    check_auth_and_forced(&mut guard, &headers)?;
    guard.requests.push(CapturedRequest {
        path: format!("/v1/namespaces/{namespace}/query"),
        body: body.clone(),
    });

    let Some(ns) = guard.namespaces.get(&namespace) else {
        return Ok(Json(json!([])));
    };

    let rank_by = body
        .get("rank_by")
        .and_then(|v| v.as_array())
        .ok_or((StatusCode::BAD_REQUEST, "missing rank_by".into()))?;
    if rank_by.len() != 3 || rank_by[0] != "vector" || rank_by[1] != "ANN" {
        return Err((StatusCode::BAD_REQUEST, "invalid rank_by".into()));
    }
    let query_vec: Vec<f32> = rank_by[2]
        .as_array()
        .ok_or((StatusCode::BAD_REQUEST, "query vector must be array".into()))?
        .iter()
        .map(|v| {
            v.as_f64()
                .map(|f| f as f32)
                .ok_or((StatusCode::BAD_REQUEST, "non-numeric query element".into()))
        })
        .collect::<std::result::Result<_, _>>()?;

    let top_k = body
        .get("top_k")
        .and_then(|v| v.as_u64())
        .ok_or((StatusCode::BAD_REQUEST, "missing top_k".into()))? as usize;
    let include_attrs = body
        .get("include_attributes")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let filters = body.get("filters");

    let mut scored: Vec<(f32, &MockRow)> = ns
        .rows
        .values()
        .filter(|row| match filters {
            Some(f) => eval_tpuf_filter(f, &row.attributes),
            None => true,
        })
        .map(|row| (compute_distance(&ns.metric, &query_vec, &row.vector), row))
        .collect();

    scored.sort_by(|a, b| a.0.total_cmp(&b.0));
    scored.truncate(top_k);

    let out: Vec<Value> = scored
        .into_iter()
        .map(|(dist, row)| {
            if include_attrs {
                json!({
                    "id": row.id,
                    "dist": dist,
                    "attributes": row.attributes,
                })
            } else {
                json!({
                    "id": row.id,
                    "dist": dist,
                })
            }
        })
        .collect();

    Ok(Json(Value::Array(out)))
}

fn compute_distance(metric: &str, a: &[f32], b: &[f32]) -> f32 {
    match metric {
        "dot_product" => -a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>(),
        "euclidean_squared" => a.iter().zip(b).map(|(x, y)| (x - y).powi(2)).sum::<f32>(),
        _ => {
            let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
            let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
            let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
            if norm_a == 0.0 || norm_b == 0.0 {
                1.0
            } else {
                1.0 - (dot / (norm_a * norm_b))
            }
        }
    }
}

fn eval_tpuf_filter(filter: &Value, attrs: &Value) -> bool {
    let Some(arr) = filter.as_array() else {
        return false;
    };
    if arr.len() == 2 {
        let Some(op) = arr[0].as_str() else {
            return false;
        };
        let Some(children) = arr[1].as_array() else {
            return false;
        };
        return match op {
            "And" => children.iter().all(|c| eval_tpuf_filter(c, attrs)),
            "Or" => children.iter().any(|c| eval_tpuf_filter(c, attrs)),
            _ => false,
        };
    }
    if arr.len() == 3 {
        let Some(field) = arr[0].as_str() else {
            return false;
        };
        let Some(op) = arr[1].as_str() else {
            return false;
        };
        let Some(actual) = attrs.get(field) else {
            return false;
        };
        return match op {
            "Eq" => json_scalar_eq(actual, &arr[2]),
            "In" => arr[2]
                .as_array()
                .is_some_and(|vals| vals.iter().any(|v| json_scalar_eq(actual, v))),
            "Gte" => actual
                .as_f64()
                .zip(arr[2].as_f64())
                .is_some_and(|(a, b)| a >= b),
            "Lte" => actual
                .as_f64()
                .zip(arr[2].as_f64())
                .is_some_and(|(a, b)| a <= b),
            _ => false,
        };
    }
    false
}

fn json_scalar_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::String(x), Value::String(y)) => x == y,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Number(x), Value::Number(y)) => {
            if let (Some(xi), Some(yi)) = (x.as_i64(), y.as_i64()) {
                xi == yi
            } else if let (Some(xf), Some(yf)) = (x.as_f64(), y.as_f64()) {
                (xf - yf).abs() < 1e-9
            } else {
                false
            }
        }
        _ => false,
    }
}

async fn spawn_mock_server(token: &str) -> (String, SharedState) {
    let state = Arc::new(Mutex::new(MockState {
        expected_token: token.to_string(),
        ..Default::default()
    }));
    let app = Router::new()
        .route("/v1/namespaces", get(handle_list_namespaces))
        .route("/v1/namespaces/{namespace}", post(handle_write_namespace))
        .route(
            "/v1/namespaces/{namespace}/query",
            post(handle_query_namespace),
        )
        .with_state(state.clone());

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}"), state)
}

fn make_store(base_url: String, api_key: &str, prefix: &str) -> TurbopufferVectorStore {
    TurbopufferVectorStore::new(TurbopufferConfig {
        base_url,
        api_key: SecretString::from(api_key),
        namespace_prefix: prefix.to_string(),
    })
    .unwrap()
}

#[tokio::test]
async fn turbopuffer_contract_vector_store() {
    let (base_url, _state) = spawn_mock_server("tpuf-contract-secret").await;
    let store = make_store(base_url, "tpuf-contract-secret", "aether-");
    contract::vector_store(&store).await;
}

#[tokio::test]
async fn turbopuffer_config_and_store_debug_redact_api_key() {
    let secret_value = "tpuf_live_super_secret_token_98765";
    let config = TurbopufferConfig {
        base_url: "http://127.0.0.1:8080".into(),
        api_key: SecretString::from(secret_value),
        namespace_prefix: "test-".into(),
    };
    let config_dbg = format!("{config:?}");
    assert!(!config_dbg.contains(secret_value));
    assert!(config_dbg.contains("[REDACTED]"));

    let store = TurbopufferVectorStore::new(config).unwrap();
    let store_dbg = format!("{store:?}");
    assert!(!store_dbg.contains(secret_value));
    assert!(store_dbg.contains("[REDACTED]"));
}

#[tokio::test]
async fn turbopuffer_connector_metadata_and_health_check() {
    let (base_url, state) = spawn_mock_server("tpuf-health-key").await;
    let store = make_store(base_url, "tpuf-health-key", "ns_");

    assert_eq!(store.name(), "turbopuffer");
    assert!(store.capabilities().supports_filter_expr);
    assert_eq!(
        store.health_check().await.unwrap(),
        ConnectorHealth::Healthy
    );
    store.close().await.unwrap();

    // Verify prefix is applied to namespace path.
    let ns = ProjectId::new("proj-1").unwrap();
    store
        .initialize(&ns, 2, DistanceMetric::Cosine)
        .await
        .unwrap();
    store
        .upsert_batch(
            &ns,
            &[VectorRecord {
                id: MemoryId::new(),
                vector: vec![1.0, 0.0],
                metadata: json!({"k": "v"}),
            }],
        )
        .await
        .unwrap();

    let guard = state.lock().unwrap();
    assert!(guard.namespaces.contains_key("ns_proj-1"));
    assert_eq!(guard.requests.len(), 1);
    assert_eq!(guard.requests[0].path, "/v1/namespaces/ns_proj-1");
    assert_eq!(guard.requests[0].body["distance_metric"], "cosine_distance");
}

#[tokio::test]
async fn turbopuffer_distance_metrics_and_score_normalization() {
    let (base_url, state) = spawn_mock_server("tpuf-metrics-key").await;
    let store = make_store(base_url, "tpuf-metrics-key", "");

    for (metric, expected_wire) in [
        (DistanceMetric::Cosine, "cosine_distance"),
        (DistanceMetric::Dot, "dot_product"),
        (DistanceMetric::Euclidean, "euclidean_squared"),
    ] {
        let ns = ProjectId::new(format!("metric-{expected_wire}")).unwrap();
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
                        metadata: json!({"kind": "near"}),
                    },
                    VectorRecord {
                        id: far,
                        vector: vec![0.0, 1.0],
                        metadata: json!({"kind": "far"}),
                    },
                ],
            )
            .await
            .unwrap();

        let hits = store.search(&ns, &[1.0, 0.0], 2, None).await.unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].id, near);
        assert_eq!(hits[1].id, far);
        assert!(
            hits[0].score > hits[1].score,
            "expected higher score for closer match under {expected_wire}: {} vs {}",
            hits[0].score,
            hits[1].score
        );

        let guard = state.lock().unwrap();
        assert_eq!(guard.namespaces[ns.as_str()].metric, expected_wire);
    }
}

#[tokio::test]
async fn turbopuffer_filter_expr_translation_all_variants() {
    let (base_url, state) = spawn_mock_server("tpuf-filter-key").await;
    let store = make_store(base_url, "tpuf-filter-key", "");
    let ns = ProjectId::new("filter-all").unwrap();
    store
        .initialize(&ns, 2, DistanceMetric::Cosine)
        .await
        .unwrap();

    let id_a = MemoryId::new();
    let id_b = MemoryId::new();
    let id_c = MemoryId::new();
    store
        .upsert_batch(
            &ns,
            &[
                VectorRecord {
                    id: id_a,
                    vector: vec![1.0, 0.0],
                    metadata: json!({"tier": "semantic", "rank": 5, "active": true}),
                },
                VectorRecord {
                    id: id_b,
                    vector: vec![0.9, 0.1],
                    metadata: json!({"tier": "episodic", "rank": 15, "active": false}),
                },
                VectorRecord {
                    id: id_c,
                    vector: vec![0.8, 0.2],
                    metadata: json!({"tier": "procedural", "rank": 25, "active": true}),
                },
            ],
        )
        .await
        .unwrap();

    // Eq
    let eq_hits = store
        .search(
            &ns,
            &[1.0, 0.0],
            10,
            Some(&FilterExpr::Eq {
                field: "active".into(),
                value: FilterValue::Bool(false),
            }),
        )
        .await
        .unwrap();
    assert_eq!(eq_hits.len(), 1);
    assert_eq!(eq_hits[0].id, id_b);

    // Range with both gte and lte
    let range_hits = store
        .search(
            &ns,
            &[1.0, 0.0],
            10,
            Some(&FilterExpr::Range {
                field: "rank".into(),
                gte: Some(10.0),
                lte: Some(20.0),
            }),
        )
        .await
        .unwrap();
    assert_eq!(range_hits.len(), 1);
    assert_eq!(range_hits[0].id, id_b);

    // Or + In
    let or_hits = store
        .search(
            &ns,
            &[1.0, 0.0],
            10,
            Some(&FilterExpr::Or(vec![
                FilterExpr::Eq {
                    field: "tier".into(),
                    value: FilterValue::String("semantic".into()),
                },
                FilterExpr::In {
                    field: "rank".into(),
                    values: vec![FilterValue::Int(25)],
                },
            ])),
        )
        .await
        .unwrap();
    assert_eq!(or_hits.len(), 2);

    let guard = state.lock().unwrap();
    let range_req = &guard.requests[2];
    assert_eq!(range_req.path, "/v1/namespaces/filter-all/query");
    assert_eq!(range_req.body["include_attributes"], json!(true));
    assert_eq!(
        range_req.body["rank_by"],
        json!(["vector", "ANN", [1.0, 0.0]])
    );
    assert_eq!(
        range_req.body["filters"],
        json!(["And", [["rank", "Gte", 10.0], ["rank", "Lte", 20.0]]])
    );
}

#[tokio::test]
async fn turbopuffer_input_validation_and_empty_ops() {
    let err = TurbopufferVectorStore::new(TurbopufferConfig {
        base_url: "   ".into(),
        api_key: SecretString::from("k"),
        namespace_prefix: "".into(),
    })
    .unwrap_err();
    assert!(matches!(err, AetherError::InvalidInput(_)));

    let (base_url, state) = spawn_mock_server("tpuf-val-key").await;
    let store = make_store(base_url, "tpuf-val-key", "");
    let ns = ProjectId::new("val-ns").unwrap();

    assert!(matches!(
        store.initialize(&ns, 0, DistanceMetric::Cosine).await,
        Err(AetherError::InvalidInput(_))
    ));
    store
        .initialize(&ns, 3, DistanceMetric::Cosine)
        .await
        .unwrap();

    // Empty batch and empty deletes must not issue HTTP requests.
    store.upsert_batch(&ns, &[]).await.unwrap();
    store.delete(&ns, &[]).await.unwrap();
    assert!(state.lock().unwrap().requests.is_empty());

    // Query dimension mismatch returns InvalidInput without hitting HTTP.
    let q_err = store.search(&ns, &[1.0, 0.0], 5, None).await.unwrap_err();
    assert!(matches!(q_err, AetherError::InvalidInput(_)));
    assert!(state.lock().unwrap().requests.is_empty());
}

#[tokio::test]
async fn turbopuffer_http_status_error_mapping() {
    let (base_url, state) = spawn_mock_server("tpuf-err-key").await;
    let store = make_store(base_url.clone(), "tpuf-err-key", "");
    let ns = ProjectId::new("err-ns").unwrap();
    store
        .initialize(&ns, 2, DistanceMetric::Cosine)
        .await
        .unwrap();

    let rec = VectorRecord {
        id: MemoryId::new(),
        vector: vec![1.0, 0.0],
        metadata: json!({}),
    };

    for (status, expected_transient) in [
        (StatusCode::TOO_MANY_REQUESTS, true),
        (StatusCode::SERVICE_UNAVAILABLE, true),
        (StatusCode::UNAUTHORIZED, false),
        (StatusCode::BAD_REQUEST, false),
    ] {
        {
            let mut guard = state.lock().unwrap();
            guard.forced_error = Some((status, format!("forced {}", status.as_u16())));
        }
        let err = store
            .upsert_batch(&ns, std::slice::from_ref(&rec))
            .await
            .unwrap_err();
        if expected_transient {
            assert!(
                matches!(
                    err,
                    AetherError::Transient {
                        connector: "turbopuffer",
                        ..
                    }
                ),
                "expected Transient for {status}, got {err:?}"
            );
            assert!(err.is_retryable());
        } else {
            assert!(
                matches!(
                    err,
                    AetherError::Permanent {
                        connector: "turbopuffer",
                        ..
                    }
                ),
                "expected Permanent for {status}, got {err:?}"
            );
            assert!(!err.is_retryable());
        }
    }

    // Wrong API key triggers real HTTP 401 -> Permanent on search as well.
    let bad_auth_store = make_store(base_url, "wrong-key", "");
    bad_auth_store
        .initialize(&ns, 2, DistanceMetric::Cosine)
        .await
        .unwrap();
    let auth_err = bad_auth_store
        .search(&ns, &[1.0, 0.0], 5, None)
        .await
        .unwrap_err();
    assert!(matches!(
        auth_err,
        AetherError::Permanent {
            connector: "turbopuffer",
            ..
        }
    ));

    let empty_range_err = bad_auth_store
        .search(
            &ns,
            &[1.0, 0.0],
            5,
            Some(&FilterExpr::Range {
                field: "age".into(),
                gte: None,
                lte: None,
            }),
        )
        .await
        .unwrap_err();
    assert!(matches!(empty_range_err, AetherError::InvalidInput(_)));
}
