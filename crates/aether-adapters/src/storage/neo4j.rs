//! Neo4j `GraphStore` connector using the HTTP Query API v2.

use std::collections::{HashMap, HashSet, VecDeque};

use async_trait::async_trait;
use chrono::{DateTime, SecondsFormat, Utc};
use nytheris_aether_core::{
    AetherError, ConnectorCapabilities, ConnectorHealth, Entity, EntityId, GraphEdge, GraphStore,
    MemoryId, OutboundConnector, ProjectId, Relation, Result, SecretString, Subgraph,
    TemporalFilter,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::storage::http;

const CONNECTOR_NAME: &str = "neo4j";
const DEFAULT_MAX_BATCH_SIZE: usize = 1000;

/// Configuration for connecting to Neo4j over the HTTP Query API v2.
#[derive(Debug, Clone)]
pub struct Neo4jConfig {
    pub base_url: String,
    pub database: String,
    pub username: String,
    pub password: SecretString,
}

/// Neo4j-backed `GraphStore` implementation communicating via `POST /db/{database}/query/v2`.
#[derive(Debug, Clone)]
pub struct Neo4jGraphStore {
    client: reqwest::Client,
    query_url: String,
    username: String,
    password: SecretString,
}

impl Neo4jGraphStore {
    /// Constructs a new `Neo4jGraphStore` with a default `reqwest::Client`.
    pub fn new(config: Neo4jConfig) -> Result<Self> {
        Self::with_client(config, reqwest::Client::new())
    }

    /// Constructs a new `Neo4jGraphStore` using the caller-provided `reqwest::Client`.
    pub fn with_client(config: Neo4jConfig, client: reqwest::Client) -> Result<Self> {
        let base_url = http::normalize_base_url(&config.base_url)?;
        let database = config.database.trim();
        if database.is_empty() {
            return Err(AetherError::InvalidInput(
                "database must not be empty".into(),
            ));
        }

        Ok(Self {
            client,
            query_url: format!("{base_url}/db/{database}/query/v2"),
            username: config.username,
            password: config.password,
        })
    }

    async fn execute_cypher(&self, statement: &str, parameters: Value) -> Result<QueryData> {
        let resp = self
            .client
            .post(&self.query_url)
            .basic_auth(&self.username, Some(self.password.expose_secret()))
            .header(reqwest::header::ACCEPT, "application/json")
            .json(&QueryRequest {
                statement,
                parameters,
            })
            .send()
            .await
            .map_err(|err| http::map_transport_err(CONNECTOR_NAME, err))?;

        let resp = match http::ensure_success(CONNECTOR_NAME, resp).await {
            Ok(ok_resp) => ok_resp,
            Err(err) => {
                if let Some(mapped) = try_extract_neo4j_err(&err) {
                    return Err(mapped);
                }
                return Err(err);
            }
        };

        let bytes = resp
            .bytes()
            .await
            .map_err(|err| http::map_transport_err(CONNECTOR_NAME, err))?;
        let parsed: QueryResponse =
            serde_json::from_slice(&bytes).map_err(|err| AetherError::Permanent {
                connector: CONNECTOR_NAME,
                message: format!("failed to decode Neo4j Query API response: {err}"),
            })?;

        if !parsed.errors.is_empty() {
            return Err(map_neo4j_errors(&parsed.errors));
        }

        Ok(parsed.data.unwrap_or_default())
    }
}

#[async_trait]
impl OutboundConnector for Neo4jGraphStore {
    fn name(&self) -> &'static str {
        CONNECTOR_NAME
    }

    fn capabilities(&self) -> ConnectorCapabilities {
        ConnectorCapabilities {
            max_batch_size: DEFAULT_MAX_BATCH_SIZE,
            supports_temporal: true,
            supports_filter_expr: false,
        }
    }

    async fn health_check(&self) -> Result<ConnectorHealth> {
        self.execute_cypher("RETURN 1 AS ok", json!({})).await?;
        Ok(ConnectorHealth::Healthy)
    }

    async fn close(&self) -> Result<()> {
        Ok(())
    }
}

#[async_trait]
impl GraphStore for Neo4jGraphStore {
    async fn upsert_entities(&self, ns: &ProjectId, entities: &[Entity]) -> Result<()> {
        if entities.is_empty() {
            return Ok(());
        }

        let rows: Vec<EntityParam<'_>> = entities
            .iter()
            .map(|e| EntityParam {
                id: e.id.as_str(),
                name: &e.name,
                entity_type: &e.entity_type,
            })
            .collect();

        let statement = "\
            UNWIND $entities AS e \
            MERGE (n:AetherEntity {project_id: $ns, id: e.id}) \
            SET n.name = e.name, n.entity_type = e.entity_type";

        self.execute_cypher(
            statement,
            json!({
                "ns": ns.as_str(),
                "entities": rows,
            }),
        )
        .await?;

        Ok(())
    }

    async fn upsert_relations(
        &self,
        ns: &ProjectId,
        source_memory_id: MemoryId,
        relations: &[Relation],
    ) -> Result<()> {
        if relations.is_empty() {
            return Ok(());
        }

        let rows: Vec<RelationParam<'_>> = relations
            .iter()
            .map(|r| RelationParam {
                source: r.source.as_str(),
                target: r.target.as_str(),
                relation_type: &r.relation_type,
                valid_at: format_timestamp(r.valid_at),
                invalid_at: r.invalid_at.map(format_timestamp),
            })
            .collect();

        let statement = "\
            UNWIND $relations AS r \
            MERGE (src:AetherEntity {project_id: $ns, id: r.source}) \
            ON CREATE SET src.name = r.source, src.entity_type = 'Unknown' \
            MERGE (dst:AetherEntity {project_id: $ns, id: r.target}) \
            ON CREATE SET dst.name = r.target, dst.entity_type = 'Unknown' \
            MERGE (src)-[e:AETHER_REL {project_id: $ns, source_memory_id: $mem_id, relation_type: r.relation_type}]->(dst) \
            SET e.valid_at = r.valid_at, e.invalid_at = r.invalid_at";

        self.execute_cypher(
            statement,
            json!({
                "ns": ns.as_str(),
                "mem_id": source_memory_id.to_string(),
                "relations": rows,
            }),
        )
        .await?;

        Ok(())
    }

    async fn retract_memory_facts(
        &self,
        ns: &ProjectId,
        source_memory_id: MemoryId,
        invalidated_at: DateTime<Utc>,
    ) -> Result<()> {
        let statement = "\
            MATCH ()-[e:AETHER_REL {project_id: $ns, source_memory_id: $mem_id}]->() \
            WHERE e.invalid_at IS NULL \
            SET e.invalid_at = $invalidated_at";

        self.execute_cypher(
            statement,
            json!({
                "ns": ns.as_str(),
                "mem_id": source_memory_id.to_string(),
                "invalidated_at": format_timestamp(invalidated_at),
            }),
        )
        .await?;

        Ok(())
    }

    async fn neighborhood(
        &self,
        ns: &ProjectId,
        seeds: &[EntityId],
        hops: u8,
        per_node_limit: usize,
        temporal: &TemporalFilter,
    ) -> Result<Subgraph> {
        if seeds.is_empty() {
            return Ok(Subgraph::default());
        }

        let seed_ids: Vec<&str> = seeds.iter().map(EntityId::as_str).collect();

        // Neo4j rejects `*1..0` bounds; 0-hop queries fetch only existing seed entities.
        if hops == 0 || per_node_limit == 0 {
            let statement = "\
                MATCH (seed:AetherEntity {project_id: $ns}) \
                WHERE seed.id IN $seeds \
                RETURN {id: seed.id, name: seed.name, entity_type: seed.entity_type} AS seed";

            let data = self
                .execute_cypher(
                    statement,
                    json!({
                        "ns": ns.as_str(),
                        "seeds": seed_ids,
                    }),
                )
                .await?;

            return decode_seed_only_subgraph(data);
        }

        let statement = format!(
            "MATCH (seed:AetherEntity {{project_id: $ns}}) \
             WHERE seed.id IN $seeds \
             OPTIONAL MATCH path = (seed)-[rels:AETHER_REL*1..{hops}]-(neighbor:AetherEntity {{project_id: $ns}}) \
             WHERE ALL(r IN rels WHERE \
               r.project_id = $ns \
               AND ($active_only = false OR r.invalid_at IS NULL) \
               AND ($as_of IS NULL OR (datetime(r.valid_at) <= datetime($as_of) AND (r.invalid_at IS NULL OR datetime($as_of) < datetime(r.invalid_at))))) \
             WITH seed, [p IN collect(path) WHERE p IS NOT NULL] AS raw_paths \
             RETURN \
               {{id: seed.id, name: seed.name, entity_type: seed.entity_type}} AS seed, \
               [p IN raw_paths | {{ \
                 nodes: [n IN nodes(p) | {{id: n.id, name: n.name, entity_type: n.entity_type}}], \
                 edges: [r IN relationships(p) | {{ \
                   source: startNode(r).id, \
                   target: endNode(r).id, \
                   relation_type: r.relation_type, \
                   source_memory_id: r.source_memory_id, \
                   valid_at: r.valid_at, \
                   invalid_at: r.invalid_at \
                 }}] \
               }}] AS paths"
        );

        let data = self
            .execute_cypher(
                &statement,
                json!({
                    "ns": ns.as_str(),
                    "seeds": seed_ids,
                    "per_node_limit": per_node_limit,
                    "active_only": temporal.active_only,
                    "as_of": temporal.as_of.map(format_timestamp),
                }),
            )
            .await?;

        decode_and_bound_subgraph(data, seeds, hops, per_node_limit)
    }
}

#[derive(Debug, Serialize)]
struct QueryRequest<'a> {
    statement: &'a str,
    parameters: Value,
}

#[derive(Debug, Serialize)]
struct EntityParam<'a> {
    id: &'a str,
    name: &'a str,
    entity_type: &'a str,
}

#[derive(Debug, Serialize)]
struct RelationParam<'a> {
    source: &'a str,
    target: &'a str,
    relation_type: &'a str,
    valid_at: String,
    invalid_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct QueryResponse {
    #[serde(default)]
    data: Option<QueryData>,
    #[serde(default)]
    errors: Vec<Neo4jError>,
}

#[derive(Debug, Default, Deserialize)]
struct QueryData {
    #[serde(default)]
    fields: Vec<String>,
    #[serde(default)]
    values: Vec<Vec<Value>>,
}

#[derive(Debug, Deserialize)]
struct Neo4jError {
    code: String,
    message: String,
}

#[derive(Debug, Deserialize)]
struct WireEntity {
    id: String,
    name: String,
    entity_type: String,
}

#[derive(Debug, Deserialize)]
struct WireEdge {
    source: String,
    target: String,
    relation_type: String,
    source_memory_id: String,
    valid_at: String,
    invalid_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WirePath {
    #[serde(default)]
    nodes: Vec<WireEntity>,
    #[serde(default)]
    edges: Vec<WireEdge>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct EdgeKey {
    source: EntityId,
    target: EntityId,
    relation_type: String,
    source_memory_id: MemoryId,
}

impl EdgeKey {
    fn from_edge(edge: &GraphEdge) -> Self {
        Self {
            source: edge.source.clone(),
            target: edge.target.clone(),
            relation_type: edge.relation_type.clone(),
            source_memory_id: edge.source_memory_id,
        }
    }
}

fn format_timestamp(ts: DateTime<Utc>) -> String {
    // Fixed nanosecond UTC formatting keeps RFC3339 lexicographical order aligned with time.
    ts.to_rfc3339_opts(SecondsFormat::Nanos, true)
}

fn parse_timestamp(raw: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(raw)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|err| AetherError::Permanent {
            connector: CONNECTOR_NAME,
            message: format!("invalid RFC3339 timestamp `{raw}`: {err}"),
        })
}

fn map_neo4j_errors(errors: &[Neo4jError]) -> AetherError {
    let message = errors
        .iter()
        .map(|e| format!("{}: {}", e.code, e.message))
        .collect::<Vec<_>>()
        .join("; ");
    if errors
        .iter()
        .any(|e| e.code.starts_with("Neo.TransientError."))
    {
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

fn try_extract_neo4j_err(err: &AetherError) -> Option<AetherError> {
    let AetherError::Permanent { message, .. } = err else {
        return None;
    };
    let (_, body) = message.split_once(": ")?;
    let parsed: QueryResponse = serde_json::from_str(body).ok()?;
    if parsed.errors.is_empty() {
        return None;
    }
    Some(map_neo4j_errors(&parsed.errors))
}

fn convert_entity(raw: WireEntity) -> Result<Entity> {
    Ok(Entity {
        id: EntityId::new(raw.id)?,
        name: raw.name,
        entity_type: raw.entity_type,
    })
}

fn convert_edge(raw: WireEdge) -> Result<GraphEdge> {
    let source = EntityId::new(raw.source)?;
    let target = EntityId::new(raw.target)?;
    let source_memory_id = Uuid::parse_str(&raw.source_memory_id)
        .map(MemoryId::from_uuid)
        .map_err(|err| AetherError::Permanent {
            connector: CONNECTOR_NAME,
            message: format!("invalid source_memory_id `{}`: {err}", raw.source_memory_id),
        })?;
    let valid_at = parse_timestamp(&raw.valid_at)?;
    let invalid_at = raw.invalid_at.as_deref().map(parse_timestamp).transpose()?;

    Ok(GraphEdge {
        source,
        target,
        relation_type: raw.relation_type,
        source_memory_id,
        valid_at,
        invalid_at,
    })
}

fn field_index(fields: &[String], name: &str, default: usize) -> usize {
    fields.iter().position(|f| f == name).unwrap_or(default)
}

fn decode_seed_only_subgraph(data: QueryData) -> Result<Subgraph> {
    let seed_idx = field_index(&data.fields, "seed", 0);
    let mut seen = HashSet::new();
    let mut entities = Vec::new();

    for row in data.values {
        let Some(val) = row.into_iter().nth(seed_idx) else {
            continue;
        };
        let wire: WireEntity =
            serde_json::from_value(val).map_err(|err| AetherError::Permanent {
                connector: CONNECTOR_NAME,
                message: format!("failed to decode seed entity: {err}"),
            })?;
        let entity = convert_entity(wire)?;
        if seen.insert(entity.id.clone()) {
            entities.push(entity);
        }
    }

    Ok(Subgraph {
        entities,
        edges: Vec::new(),
    })
}

fn decode_and_bound_subgraph(
    data: QueryData,
    seeds: &[EntityId],
    hops: u8,
    per_node_limit: usize,
) -> Result<Subgraph> {
    let seed_idx = field_index(&data.fields, "seed", 0);
    let paths_idx = field_index(&data.fields, "paths", 1);

    let mut entity_map: HashMap<EntityId, Entity> = HashMap::new();
    let mut unique_edges: Vec<GraphEdge> = Vec::new();
    let mut seen_edge_keys: HashSet<EdgeKey> = HashSet::new();

    for row in data.values {
        if let Some(seed_val) = row.get(seed_idx).cloned() {
            let wire: WireEntity =
                serde_json::from_value(seed_val).map_err(|err| AetherError::Permanent {
                    connector: CONNECTOR_NAME,
                    message: format!("failed to decode seed entity: {err}"),
                })?;
            let ent = convert_entity(wire)?;
            entity_map.entry(ent.id.clone()).or_insert(ent);
        }

        if let Some(paths_val) = row.get(paths_idx).cloned() {
            let paths: Vec<WirePath> =
                serde_json::from_value(paths_val).map_err(|err| AetherError::Permanent {
                    connector: CONNECTOR_NAME,
                    message: format!("failed to decode neighborhood paths: {err}"),
                })?;
            for path in paths {
                for node in path.nodes {
                    let ent = convert_entity(node)?;
                    entity_map.entry(ent.id.clone()).or_insert(ent);
                }
                for edge in path.edges {
                    let converted = convert_edge(edge)?;
                    let key = EdgeKey::from_edge(&converted);
                    if seen_edge_keys.insert(key) {
                        unique_edges.push(converted);
                    }
                }
            }
        }
    }

    let mut visited_nodes: HashSet<EntityId> = HashSet::new();
    let mut ordered_nodes: Vec<EntityId> = Vec::new();
    let mut visited_edges: HashSet<EdgeKey> = HashSet::new();
    let mut queue: VecDeque<(EntityId, u8)> = VecDeque::new();

    for seed in seeds {
        if entity_map.contains_key(seed) && visited_nodes.insert(seed.clone()) {
            ordered_nodes.push(seed.clone());
            queue.push_back((seed.clone(), 0));
        }
    }

    let mut collected_edges = Vec::new();
    while let Some((node_id, depth)) = queue.pop_front() {
        if depth >= hops {
            continue;
        }
        let mut per_node_count = 0usize;
        for edge in &unique_edges {
            if per_node_count >= per_node_limit {
                break;
            }
            let neighbor = if edge.source == node_id {
                &edge.target
            } else if edge.target == node_id {
                &edge.source
            } else {
                continue;
            };
            if !visited_edges.insert(EdgeKey::from_edge(edge)) {
                continue;
            }
            collected_edges.push(edge.clone());
            per_node_count += 1;

            if entity_map.contains_key(neighbor) && visited_nodes.insert(neighbor.clone()) {
                ordered_nodes.push(neighbor.clone());
                queue.push_back((neighbor.clone(), depth + 1));
            }
        }
    }

    let entities = ordered_nodes
        .into_iter()
        .filter_map(|id| entity_map.remove(&id))
        .collect();

    Ok(Subgraph {
        entities,
        edges: collected_edges,
    })
}
