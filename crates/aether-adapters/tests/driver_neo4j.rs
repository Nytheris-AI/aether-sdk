#![cfg(feature = "neo4j")]

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use chrono::{DateTime, Duration as ChronoDuration, SecondsFormat, Utc};
use nytheris_aether_adapters::contract;
use nytheris_aether_adapters::middleware::{Resilient, RetryPolicy};
use nytheris_aether_adapters::storage::neo4j::{Neo4jConfig, Neo4jGraphStore};
use nytheris_aether_core::{
    AetherError, ConnectorHealth, Entity, EntityId, GraphEdge, GraphStore, MemoryId,
    OutboundConnector, ProjectId, Relation, SecretString, TemporalFilter,
};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::net::TcpListener;

#[derive(Debug, Clone)]
struct RecordedQuery {
    database: String,
    statement: String,
    parameters: Value,
}

#[derive(Debug, Clone)]
enum FaultInjection {
    HttpStatus(StatusCode, String),
    Neo4jError {
        status: StatusCode,
        code: String,
        message: String,
    },
}

#[derive(Default)]
struct ProjectGraph {
    entities: HashMap<String, Entity>,
    edges: Vec<GraphEdge>,
}

struct MockNeo4jState {
    expected_database: String,
    expected_auth_header: String,
    projects: HashMap<String, ProjectGraph>,
    recorded: Vec<RecordedQuery>,
    faults: VecDeque<FaultInjection>,
}

impl MockNeo4jState {
    fn new(database: &str, username: &str, password: &str) -> Self {
        let token = base64_encode(format!("{username}:{password}").as_bytes());
        Self {
            expected_database: database.to_string(),
            expected_auth_header: format!("Basic {token}"),
            projects: HashMap::new(),
            recorded: Vec::new(),
            faults: VecDeque::new(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct IncomingQuery {
    statement: String,
    #[serde(default)]
    parameters: Value,
}

#[derive(Debug, Deserialize)]
struct EntityParam {
    id: String,
    name: String,
    entity_type: String,
}

#[derive(Debug, Deserialize)]
struct RelationParam {
    source: String,
    target: String,
    relation_type: String,
    valid_at: String,
    invalid_at: Option<String>,
}

fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((triple >> 18) & 0x3f) as usize] as char);
        out.push(TABLE[((triple >> 12) & 0x3f) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[((triple >> 6) & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(triple & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

async fn spawn_mock_neo4j(
    database: &str,
    username: &str,
    password: &str,
) -> (String, Arc<Mutex<MockNeo4jState>>) {
    let state = Arc::new(Mutex::new(MockNeo4jState::new(
        database, username, password,
    )));
    let app = Router::new()
        .route("/db/{database}/query/v2", post(handle_query_v2))
        .with_state(state.clone());

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}"), state)
}

fn neo4j_err_response(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        Json(json!({
            "errors": [
                { "code": code, "message": message }
            ]
        })),
    )
        .into_response()
}

async fn handle_query_v2(
    State(state): State<Arc<Mutex<MockNeo4jState>>>,
    Path(database): Path<String>,
    headers: HeaderMap,
    Json(req): Json<IncomingQuery>,
) -> Response {
    let mut guard = state.lock().unwrap();

    let auth = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    if auth != Some(guard.expected_auth_header.as_str()) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({
                "errors": [
                    {
                        "code": "Neo.ClientError.Security.Unauthorized",
                        "message": "Invalid username or password"
                    }
                ]
            })),
        )
            .into_response();
    }

    if database != guard.expected_database {
        return neo4j_err_response(
            StatusCode::NOT_FOUND,
            "Neo.ClientError.Database.DatabaseNotFound",
            "Database does not exist",
        );
    }

    guard.recorded.push(RecordedQuery {
        database,
        statement: req.statement.clone(),
        parameters: req.parameters.clone(),
    });

    if let Some(fault) = guard.faults.pop_front() {
        return match fault {
            FaultInjection::HttpStatus(status, body) => (status, body).into_response(),
            FaultInjection::Neo4jError {
                status,
                code,
                message,
            } => neo4j_err_response(status, &code, &message),
        };
    }

    let stmt = req.statement.as_str();
    let params = &req.parameters;

    if stmt.trim().starts_with("RETURN 1") {
        return (
            StatusCode::OK,
            Json(json!({
                "data": {
                    "fields": ["ok"],
                    "values": [[1]]
                }
            })),
        )
            .into_response();
    }

    if stmt.contains("UNWIND $entities") {
        let ns = params["ns"].as_str().unwrap().to_string();
        let entities: Vec<EntityParam> =
            serde_json::from_value(params["entities"].clone()).unwrap();
        let proj = guard.projects.entry(ns).or_default();
        for e in entities {
            proj.entities.insert(
                e.id.clone(),
                Entity {
                    id: EntityId::new(e.id).unwrap(),
                    name: e.name,
                    entity_type: e.entity_type,
                },
            );
        }
        return (
            StatusCode::OK,
            Json(json!({ "data": { "fields": [], "values": [] } })),
        )
            .into_response();
    }

    if stmt.contains("UNWIND $relations") {
        let ns = params["ns"].as_str().unwrap().to_string();
        let mem_id_str = params["mem_id"].as_str().unwrap();
        let mem_id = MemoryId::from_uuid(mem_id_str.parse().unwrap());
        let relations: Vec<RelationParam> =
            serde_json::from_value(params["relations"].clone()).unwrap();
        let proj = guard.projects.entry(ns).or_default();
        for r in relations {
            proj.entities
                .entry(r.source.clone())
                .or_insert_with(|| Entity {
                    id: EntityId::new(r.source.clone()).unwrap(),
                    name: r.source.clone(),
                    entity_type: "Unknown".into(),
                });
            proj.entities
                .entry(r.target.clone())
                .or_insert_with(|| Entity {
                    id: EntityId::new(r.target.clone()).unwrap(),
                    name: r.target.clone(),
                    entity_type: "Unknown".into(),
                });
            let valid_at = DateTime::parse_from_rfc3339(&r.valid_at)
                .unwrap()
                .with_timezone(&Utc);
            let invalid_at = r
                .invalid_at
                .as_deref()
                .map(|s| DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc));
            let source_id = EntityId::new(r.source).unwrap();
            let target_id = EntityId::new(r.target).unwrap();

            if let Some(existing) = proj.edges.iter_mut().find(|e| {
                e.source == source_id
                    && e.target == target_id
                    && e.relation_type == r.relation_type
                    && e.source_memory_id == mem_id
            }) {
                existing.valid_at = valid_at;
                existing.invalid_at = invalid_at;
            } else {
                proj.edges.push(GraphEdge {
                    source: source_id,
                    target: target_id,
                    relation_type: r.relation_type,
                    source_memory_id: mem_id,
                    valid_at,
                    invalid_at,
                });
            }
        }
        return (
            StatusCode::OK,
            Json(json!({ "data": { "fields": [], "values": [] } })),
        )
            .into_response();
    }

    if stmt.contains("e.invalid_at = $invalidated_at") {
        let ns = params["ns"].as_str().unwrap();
        let mem_id = MemoryId::from_uuid(params["mem_id"].as_str().unwrap().parse().unwrap());
        let invalidated_at =
            DateTime::parse_from_rfc3339(params["invalidated_at"].as_str().unwrap())
                .unwrap()
                .with_timezone(&Utc);
        if let Some(proj) = guard.projects.get_mut(ns) {
            for edge in &mut proj.edges {
                if edge.source_memory_id == mem_id && edge.invalid_at.is_none() {
                    edge.invalid_at = Some(invalidated_at);
                }
            }
        }
        return (
            StatusCode::OK,
            Json(json!({ "data": { "fields": [], "values": [] } })),
        )
            .into_response();
    }

    if stmt.contains("WHERE seed.id IN $seeds") {
        let ns = params["ns"].as_str().unwrap();
        let seeds: Vec<String> = serde_json::from_value(params["seeds"].clone()).unwrap();
        let Some(proj) = guard.projects.get(ns) else {
            let fields = if stmt.contains("*1..") {
                json!(["seed", "paths"])
            } else {
                json!(["seed"])
            };
            return (
                StatusCode::OK,
                Json(json!({ "data": { "fields": fields, "values": [] } })),
            )
                .into_response();
        };

        if let Some(hops) = parse_hops_bound(stmt) {
            let active_only = params["active_only"].as_bool().unwrap();
            let as_of = params["as_of"]
                .as_str()
                .map(|s| DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc));
            let per_node_limit = params["per_node_limit"].as_u64().unwrap() as usize;

            let slice_per_seed = stmt.contains("[0..$per_node_limit]");
            let rows = execute_mock_neighborhood(
                proj,
                &seeds,
                hops,
                per_node_limit,
                slice_per_seed,
                &TemporalFilter { as_of, active_only },
            );
            return (
                StatusCode::OK,
                Json(json!({
                    "data": {
                        "fields": ["seed", "paths"],
                        "values": rows
                    }
                })),
            )
                .into_response();
        }

        let mut rows = Vec::new();
        for seed_id in seeds {
            if let Some(ent) = proj.entities.get(&seed_id) {
                rows.push(vec![json!({
                    "id": ent.id.as_str(),
                    "name": ent.name,
                    "entity_type": ent.entity_type,
                })]);
            }
        }
        return (
            StatusCode::OK,
            Json(json!({
                "data": {
                    "fields": ["seed"],
                    "values": rows
                }
            })),
        )
            .into_response();
    }

    neo4j_err_response(
        StatusCode::BAD_REQUEST,
        "Neo.ClientError.Statement.SyntaxError",
        &format!("Unhandled test Cypher statement: {stmt}"),
    )
}

fn parse_hops_bound(stmt: &str) -> Option<u8> {
    let idx = stmt.find("*1..")?;
    let rest = &stmt[idx + 4..];
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse::<u8>().ok()
}

fn edge_matches_temporal(edge: &GraphEdge, temporal: &TemporalFilter) -> bool {
    if let Some(as_of) = temporal.as_of {
        if edge.valid_at > as_of {
            return false;
        }
        if let Some(inv) = edge.invalid_at {
            if as_of >= inv {
                return false;
            }
        }
    }
    !(temporal.active_only && edge.invalid_at.is_some())
}

fn execute_mock_neighborhood(
    proj: &ProjectGraph,
    seeds: &[String],
    hops: u8,
    per_node_limit: usize,
    slice_per_seed: bool,
    temporal: &TemporalFilter,
) -> Vec<Vec<Value>> {
    let mut rows = Vec::new();
    for seed_id in seeds {
        let Some(seed_ent) = proj.entities.get(seed_id) else {
            continue;
        };

        let mut paths = Vec::new();
        // Enumerate simple edge paths of length 1..=hops from seed_ent.
        type PathStep = (String, Vec<String>, Vec<GraphEdge>);
        let mut queue: VecDeque<PathStep> = VecDeque::new();
        queue.push_back((seed_id.clone(), vec![seed_id.clone()], Vec::new()));

        while let Some((curr, node_seq, edge_seq)) = queue.pop_front() {
            if edge_seq.len() == hops as usize {
                continue;
            }
            for edge in &proj.edges {
                if !edge_matches_temporal(edge, temporal) {
                    continue;
                }
                let next = if edge.source.as_str() == curr {
                    edge.target.as_str().to_string()
                } else if edge.target.as_str() == curr {
                    edge.source.as_str().to_string()
                } else {
                    continue;
                };
                // Avoid repeating exact edge in the same path.
                let already_in_path = edge_seq.iter().any(|e| {
                    e.source == edge.source
                        && e.target == edge.target
                        && e.relation_type == edge.relation_type
                        && e.source_memory_id == edge.source_memory_id
                });
                if already_in_path {
                    continue;
                }
                let mut next_nodes = node_seq.clone();
                next_nodes.push(next.clone());
                let mut next_edges = edge_seq.clone();
                next_edges.push(edge.clone());

                let path_nodes: Vec<Value> = next_nodes
                    .iter()
                    .filter_map(|id| proj.entities.get(id))
                    .map(|ent| {
                        json!({
                            "id": ent.id.as_str(),
                            "name": ent.name,
                            "entity_type": ent.entity_type,
                        })
                    })
                    .collect();
                let path_edges: Vec<Value> = next_edges
                    .iter()
                    .map(|e| {
                        json!({
                            "source": e.source.as_str(),
                            "target": e.target.as_str(),
                            "relation_type": e.relation_type,
                            "source_memory_id": e.source_memory_id.to_string(),
                            "valid_at": e.valid_at.to_rfc3339_opts(SecondsFormat::Nanos, true),
                            "invalid_at": e.invalid_at.map(|dt| dt.to_rfc3339_opts(SecondsFormat::Nanos, true)),
                        })
                    })
                    .collect();
                paths.push(json!({
                    "nodes": path_nodes,
                    "edges": path_edges,
                }));
                queue.push_back((next, next_nodes, next_edges));
            }
        }

        if slice_per_seed {
            paths.truncate(per_node_limit);
        }

        rows.push(vec![
            json!({
                "id": seed_ent.id.as_str(),
                "name": seed_ent.name,
                "entity_type": seed_ent.entity_type,
            }),
            Value::Array(paths),
        ]);
    }
    rows
}

fn make_store(base_url: String) -> Neo4jGraphStore {
    Neo4jGraphStore::new(Neo4jConfig {
        base_url,
        database: "neo4j".into(),
        username: "neo4j".into(),
        password: SecretString::new("test-password"),
    })
    .unwrap()
}

#[cfg(feature = "contract-suite")]
#[tokio::test]
async fn neo4j_contract_graph_store() {
    let (base_url, _state) = spawn_mock_neo4j("neo4j", "neo4j", "test-password").await;
    let store = make_store(base_url);
    contract::graph_store(&store).await;
}

#[tokio::test]
async fn neo4j_connector_metadata_config_validation_and_health() {
    let cfg = Neo4jConfig {
        base_url: "http://127.0.0.1:7474".into(),
        database: "neo4j".into(),
        username: "neo4j".into(),
        password: SecretString::new("super-secret-password"),
    };
    let debug_str = format!("{cfg:?}");
    assert!(!debug_str.contains("super-secret-password"));

    let empty_url = Neo4jGraphStore::new(Neo4jConfig {
        base_url: "   ".into(),
        ..cfg.clone()
    });
    assert!(matches!(empty_url, Err(AetherError::InvalidInput(_))));

    let empty_db = Neo4jGraphStore::new(Neo4jConfig {
        database: "".into(),
        ..cfg
    });
    assert!(matches!(empty_db, Err(AetherError::InvalidInput(_))));

    let (base_url, _state) = spawn_mock_neo4j("neo4j", "neo4j", "test-password").await;
    let store = make_store(format!("{base_url}/"));
    assert_eq!(store.name(), "neo4j");
    assert!(store.capabilities().supports_temporal);
    assert_eq!(
        store.health_check().await.unwrap(),
        ConnectorHealth::Healthy
    );
    store.close().await.unwrap();
}

#[tokio::test]
async fn neo4j_cypher_parameter_generation() {
    let (base_url, state) = spawn_mock_neo4j("testdb", "alice", "secret").await;
    let store = Neo4jGraphStore::new(Neo4jConfig {
        base_url,
        database: "testdb".into(),
        username: "alice".into(),
        password: SecretString::new("secret"),
    })
    .unwrap();

    let ns = ProjectId::new("proj-params").unwrap();
    let e1 = Entity {
        id: EntityId::new("person:alice").unwrap(),
        name: "Alice".into(),
        entity_type: "Person".into(),
    };
    let e2 = Entity {
        id: EntityId::new("db:neo4j").unwrap(),
        name: "Neo4j".into(),
        entity_type: "Database".into(),
    };
    store
        .upsert_entities(&ns, &[e1.clone(), e2.clone()])
        .await
        .unwrap();

    let mem_id = MemoryId::new();
    let t0 = Utc::now() - ChronoDuration::minutes(5);
    let t1 = Utc::now();
    store
        .upsert_relations(
            &ns,
            mem_id,
            &[Relation {
                source: e1.id.clone(),
                target: e2.id.clone(),
                relation_type: "USES".into(),
                valid_at: t0,
                invalid_at: None,
            }],
        )
        .await
        .unwrap();

    store.retract_memory_facts(&ns, mem_id, t1).await.unwrap();

    let as_of = t0 + ChronoDuration::minutes(1);
    let _sub = store
        .neighborhood(
            &ns,
            &[e1.id],
            2,
            5,
            &TemporalFilter {
                as_of: Some(as_of),
                active_only: false,
            },
        )
        .await
        .unwrap();

    let recorded = state.lock().unwrap().recorded.clone();
    assert_eq!(recorded.len(), 4);
    assert!(recorded.iter().all(|r| r.database == "testdb"));

    // 1. upsert_entities
    assert!(recorded[0].statement.contains("UNWIND $entities AS e"));
    assert!(recorded[0]
        .statement
        .contains("MERGE (n:AetherEntity {project_id: $ns, id: e.id})"));
    assert_eq!(recorded[0].parameters["ns"], "proj-params");
    assert_eq!(
        recorded[0].parameters["entities"].as_array().unwrap().len(),
        2
    );

    // 2. upsert_relations
    assert!(recorded[1].statement.contains("UNWIND $relations AS r"));
    assert!(recorded[1].statement.contains("MERGE"));
    assert_eq!(recorded[1].parameters["ns"], "proj-params");
    assert_eq!(recorded[1].parameters["mem_id"], mem_id.to_string());
    let rel_param = &recorded[1].parameters["relations"][0];
    assert_eq!(rel_param["relation_type"], "USES");
    assert!(rel_param["valid_at"].as_str().is_some());
    assert!(rel_param["invalid_at"].is_null());

    // 3. retract_memory_facts
    assert!(recorded[2]
        .statement
        .contains("e.invalid_at = $invalidated_at"));
    assert_eq!(recorded[2].parameters["ns"], "proj-params");
    assert_eq!(recorded[2].parameters["mem_id"], mem_id.to_string());
    assert!(recorded[2].parameters["invalidated_at"].as_str().is_some());

    // 4. neighborhood
    assert!(recorded[3].statement.contains("*1..2"));
    assert_eq!(recorded[3].parameters["ns"], "proj-params");
    assert_eq!(recorded[3].parameters["seeds"], json!(["person:alice"]));
    assert_eq!(recorded[3].parameters["per_node_limit"], 5);
    assert_eq!(recorded[3].parameters["active_only"], false);
    assert!(recorded[3].parameters["as_of"].as_str().is_some());
}

#[tokio::test]
async fn neo4j_neighborhood_hops_zero_multi_hop_and_limits() {
    let (base_url, state) = spawn_mock_neo4j("neo4j", "neo4j", "test-password").await;
    let store = make_store(base_url);
    let ns = ProjectId::new("proj-hops").unwrap();

    let a = Entity {
        id: EntityId::new("n:a").unwrap(),
        name: "A".into(),
        entity_type: "Node".into(),
    };
    let b = Entity {
        id: EntityId::new("n:b").unwrap(),
        name: "B".into(),
        entity_type: "Node".into(),
    };
    let c = Entity {
        id: EntityId::new("n:c").unwrap(),
        name: "C".into(),
        entity_type: "Node".into(),
    };
    let d = Entity {
        id: EntityId::new("n:d").unwrap(),
        name: "D".into(),
        entity_type: "Node".into(),
    };
    store
        .upsert_entities(&ns, &[a.clone(), b.clone(), c.clone(), d.clone()])
        .await
        .unwrap();

    let now = Utc::now();
    store
        .upsert_relations(
            &ns,
            MemoryId::new(),
            &[
                Relation {
                    source: a.id.clone(),
                    target: b.id.clone(),
                    relation_type: "LINK".into(),
                    valid_at: now,
                    invalid_at: None,
                },
                Relation {
                    source: b.id.clone(),
                    target: c.id.clone(),
                    relation_type: "LINK".into(),
                    valid_at: now,
                    invalid_at: None,
                },
                Relation {
                    source: a.id.clone(),
                    target: d.id.clone(),
                    relation_type: "OTHER".into(),
                    valid_at: now,
                    invalid_at: None,
                },
            ],
        )
        .await
        .unwrap();

    // hops == 0 returns only seed entities and no edges, without *1..0 Cypher.
    let sub0 = store
        .neighborhood(
            &ns,
            std::slice::from_ref(&a.id),
            0,
            10,
            &TemporalFilter::default(),
        )
        .await
        .unwrap();
    assert_eq!(sub0.entities, vec![a.clone()]);
    assert!(sub0.edges.is_empty());
    let last_stmt = state
        .lock()
        .unwrap()
        .recorded
        .last()
        .unwrap()
        .statement
        .clone();
    assert!(!last_stmt.contains("*1..0"));

    // Empty inputs short-circuit without errors.
    store.upsert_entities(&ns, &[]).await.unwrap();
    store
        .upsert_relations(&ns, MemoryId::new(), &[])
        .await
        .unwrap();
    let empty_sub = store
        .neighborhood(&ns, &[], 2, 10, &TemporalFilter::default())
        .await
        .unwrap();
    assert!(empty_sub.entities.is_empty());
    assert!(empty_sub.edges.is_empty());

    // hops == 1 with per_node_limit == 1 limits edges from A to 1.
    let sub1 = store
        .neighborhood(
            &ns,
            std::slice::from_ref(&a.id),
            1,
            1,
            &TemporalFilter::default(),
        )
        .await
        .unwrap();
    assert_eq!(sub1.edges.len(), 1);
    assert_eq!(sub1.entities.len(), 2);

    // hops == 2 reaches C through B.
    let sub2 = store
        .neighborhood(&ns, &[a.id], 2, 10, &TemporalFilter::default())
        .await
        .unwrap();
    assert_eq!(sub2.edges.len(), 3);
    let ids: HashSet<String> = sub2
        .entities
        .into_iter()
        .map(|e| e.id.as_str().to_string())
        .collect();
    assert_eq!(
        ids,
        HashSet::from([
            "n:a".to_string(),
            "n:b".to_string(),
            "n:c".to_string(),
            "n:d".to_string()
        ])
    );
}

#[tokio::test]
async fn neo4j_error_mapping_transient_and_permanent() {
    let (base_url, state) = spawn_mock_neo4j("neo4j", "neo4j", "test-password").await;
    let store = make_store(base_url.clone());
    let ns = ProjectId::new("proj-err").unwrap();
    let entity = Entity {
        id: EntityId::new("person:bob").unwrap(),
        name: "Bob".into(),
        entity_type: "Person".into(),
    };

    // 1. Neo.TransientError.* in JSON errors array -> Transient
    state
        .lock()
        .unwrap()
        .faults
        .push_back(FaultInjection::Neo4jError {
            status: StatusCode::OK,
            code: "Neo.TransientError.Transaction.DeadlockDetected".into(),
            message: "ForsetiClient deadlock".into(),
        });
    let err = store
        .upsert_entities(&ns, std::slice::from_ref(&entity))
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            AetherError::Transient {
                connector: "neo4j",
                ..
            }
        ),
        "expected Transient, got {err:?}"
    );
    assert!(err.is_retryable());

    // 2. Neo.ClientError.* in JSON errors array -> Permanent
    state
        .lock()
        .unwrap()
        .faults
        .push_back(FaultInjection::Neo4jError {
            status: StatusCode::OK,
            code: "Neo.ClientError.Statement.SyntaxError".into(),
            message: "Invalid input".into(),
        });
    let err = store
        .upsert_entities(&ns, std::slice::from_ref(&entity))
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            AetherError::Permanent {
                connector: "neo4j",
                ..
            }
        ),
        "expected Permanent, got {err:?}"
    );
    assert!(!err.is_retryable());

    // 2b. HTTP 400 with Neo.TransientError.* -> still mapped to Transient
    state
        .lock()
        .unwrap()
        .faults
        .push_back(FaultInjection::Neo4jError {
            status: StatusCode::BAD_REQUEST,
            code: "Neo.TransientError.Transaction.Terminated".into(),
            message: "Transaction terminated".into(),
        });
    let err = store
        .upsert_entities(&ns, std::slice::from_ref(&entity))
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            AetherError::Transient {
                connector: "neo4j",
                ..
            }
        ),
        "expected Transient for HTTP 400 Neo.TransientError, got {err:?}"
    );

    // 3. HTTP 429 -> Transient
    state
        .lock()
        .unwrap()
        .faults
        .push_back(FaultInjection::HttpStatus(
            StatusCode::TOO_MANY_REQUESTS,
            "rate limited".into(),
        ));
    let err = store
        .upsert_entities(&ns, std::slice::from_ref(&entity))
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            AetherError::Transient {
                connector: "neo4j",
                ..
            }
        ),
        "expected Transient for 429, got {err:?}"
    );

    // 4. HTTP 503 -> Transient
    state
        .lock()
        .unwrap()
        .faults
        .push_back(FaultInjection::HttpStatus(
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable".into(),
        ));
    let err = store
        .upsert_entities(&ns, std::slice::from_ref(&entity))
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            AetherError::Transient {
                connector: "neo4j",
                ..
            }
        ),
        "expected Transient for 503, got {err:?}"
    );

    // 5. HTTP 401 (bad credentials) -> Permanent
    let bad_auth_store = Neo4jGraphStore::new(Neo4jConfig {
        base_url,
        database: "neo4j".into(),
        username: "neo4j".into(),
        password: SecretString::new("wrong-password"),
    })
    .unwrap();
    let err = bad_auth_store
        .upsert_entities(&ns, std::slice::from_ref(&entity))
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            AetherError::Permanent {
                connector: "neo4j",
                ..
            }
        ),
        "expected Permanent for 401, got {err:?}"
    );
    assert!(!err.is_retryable());

    // 6. Resilient middleware retries Transient and succeeds, but aborts on Permanent.
    state
        .lock()
        .unwrap()
        .faults
        .push_back(FaultInjection::Neo4jError {
            status: StatusCode::OK,
            code: "Neo.TransientError.Transaction.LockAcquisitionTimeout".into(),
            message: "lock timeout".into(),
        });
    let resilient = Resilient::new(
        store,
        RetryPolicy {
            max_retries: 2,
            base_delay: Duration::from_millis(1),
        },
    );
    resilient.upsert_entities(&ns, &[entity]).await.unwrap();
    let sub = resilient
        .neighborhood(
            &ns,
            &[EntityId::new("person:bob").unwrap()],
            1,
            10,
            &TemporalFilter::default(),
        )
        .await
        .unwrap();
    assert_eq!(sub.entities.len(), 1);
}

#[tokio::test]
async fn neo4j_multi_seed_per_node_limit_does_not_drop_unvisited_edges() {
    let (base_url, _state) = spawn_mock_neo4j("neo4j", "neo4j", "secret").await;
    let store = Neo4jGraphStore::new(Neo4jConfig {
        base_url,
        database: "neo4j".into(),
        username: "neo4j".into(),
        password: SecretString::new("secret"),
    })
    .unwrap();
    let ns = ProjectId::new(format!("multi-seed-{}", uuid::Uuid::new_v4())).unwrap();

    let a = Entity {
        id: EntityId::new("n:a").unwrap(),
        name: "A".into(),
        entity_type: "Node".into(),
    };
    let b = Entity {
        id: EntityId::new("n:b").unwrap(),
        name: "B".into(),
        entity_type: "Node".into(),
    };
    let c = Entity {
        id: EntityId::new("n:c").unwrap(),
        name: "C".into(),
        entity_type: "Node".into(),
    };
    store
        .upsert_entities(&ns, &[a.clone(), b.clone(), c.clone()])
        .await
        .unwrap();

    let now = chrono::Utc::now();
    store
        .upsert_relations(
            &ns,
            MemoryId::new(),
            &[
                Relation {
                    source: a.id.clone(),
                    target: b.id.clone(),
                    relation_type: "LINK".into(),
                    valid_at: now,
                    invalid_at: None,
                },
                Relation {
                    source: b.id.clone(),
                    target: c.id.clone(),
                    relation_type: "LINK".into(),
                    valid_at: now,
                    invalid_at: None,
                },
            ],
        )
        .await
        .unwrap();

    let sub = store
        .neighborhood(&ns, &[a.id, b.id], 1, 1, &TemporalFilter::default())
        .await
        .unwrap();
    assert_eq!(sub.edges.len(), 2);
}

#[tokio::test]
#[ignore = "requires live Neo4j (NEO4J_URL or default http://127.0.0.1:17474)"]
async fn live_neo4j_contract_graph_store() {
    let base_url =
        std::env::var("NEO4J_URL").unwrap_or_else(|_| "http://127.0.0.1:17474".to_string());
    let database = std::env::var("NEO4J_DATABASE").unwrap_or_else(|_| "neo4j".to_string());
    let username = std::env::var("NEO4J_USER").unwrap_or_else(|_| "neo4j".to_string());
    let password = std::env::var("NEO4J_PASSWORD").unwrap_or_else(|_| "testpassword".to_string());

    let store = Neo4jGraphStore::new(Neo4jConfig {
        base_url,
        database,
        username,
        password: SecretString::new(password),
    })
    .unwrap();

    assert_eq!(
        store.health_check().await.unwrap(),
        ConnectorHealth::Healthy
    );
    contract::graph_store(&store).await;
    store.close().await.unwrap();
}
