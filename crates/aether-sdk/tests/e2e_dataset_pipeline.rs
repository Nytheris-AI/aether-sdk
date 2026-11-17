use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use aether_adapters::{InMemoryBackend, Resilient, RetryPolicy, Timeout, Traced};
use aether_core::{
    AetherError, ConsistencyMode, DistanceMetric, Entity, EntityId, FilterExpr, FilterValue,
    MemoryId, MemoryInbound, MemoryInput, MemoryScope, MemoryTier, MemoryUpdate, ProjectId,
    RecallMode, RecallQuery, ReceiptState, Relation, TemporalFilter, VectorStore, WriteDurability,
};
use aether_sdk::MemoryManager;
use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use serde::Deserialize;
use serde_json::{json, Value};

const EMBED_DIM: usize = 64;

/// Deterministic feature-hashed token + bigram embedding normalized to unit L2 norm.
fn embed_text(text: &str) -> Vec<f32> {
    let mut vec = vec![0.0f32; EMBED_DIM];
    let tokens: Vec<String> = text
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() >= 2)
        .map(str::to_string)
        .collect();

    for (idx, tok) in tokens.iter().enumerate() {
        let h1 = fnv1a_hash(tok.as_bytes()) as usize;
        vec[h1 % EMBED_DIM] += 1.0;

        if idx + 1 < tokens.len() {
            let bigram = format!("{}_{}", tok, tokens[idx + 1]);
            let h2 = fnv1a_hash(bigram.as_bytes()) as usize;
            vec[h2 % EMBED_DIM] += 0.5;
        }
    }

    let norm: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in &mut vec {
            *x /= norm;
        }
    } else {
        vec[0] = 1.0;
    }
    vec
}

fn fnv1a_hash(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

fn slug(raw: &str) -> String {
    raw.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect()
}

/// Parses LoCoMo timestamps like `"1:56 pm on 8 May, 2023"` into UTC `DateTime`.
fn parse_locomo_datetime(raw: &str) -> DateTime<Utc> {
    if let Ok(naive) = NaiveDateTime::parse_from_str(raw.trim(), "%-I:%M %P on %-d %B, %Y") {
        return Utc.from_utc_datetime(&naive);
    }
    Utc.with_ymd_and_hms(2023, 1, 1, 0, 0, 0).unwrap()
}

#[derive(Debug, Deserialize)]
struct LocomoSample {
    sample_id: String,
    conversation: BTreeMap<String, Value>,
    #[serde(default)]
    observation: BTreeMap<String, Value>,
    qa: Vec<LocomoQa>,
}

#[derive(Debug, Deserialize)]
struct LocomoTurn {
    speaker: String,
    dia_id: String,
    text: String,
}

#[derive(Debug, Deserialize)]
struct LocomoQa {
    question: String,
    evidence: Vec<String>,
    category: u8,
}

#[derive(Debug, Deserialize)]
struct KnowledgeLifecycleFixture {
    projects: Vec<KnowledgeProject>,
}

#[derive(Debug, Deserialize)]
struct KnowledgeProject {
    project_id: String,
    records: Vec<KnowledgeRecord>,
    updates: Vec<KnowledgeUpdateSpec>,
    forget_targets: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct KnowledgeRecord {
    id: String,
    idempotency_key: String,
    tier: MemoryTier,
    scope: MemoryScope,
    content: String,
    entities: Vec<Entity>,
    relations: Vec<Relation>,
    metadata: Value,
}

#[derive(Debug, Deserialize)]
struct KnowledgeUpdateSpec {
    target_id: String,
    new_content: String,
    new_metadata: Value,
}

fn build_resilient_manager() -> (Arc<InMemoryBackend>, Arc<MemoryManager>) {
    let raw = Arc::new(InMemoryBackend::new());
    let record_store = Arc::new(Traced::new(Resilient::new(
        Timeout::new(raw.clone(), Duration::from_secs(2)),
        RetryPolicy::default(),
    )));
    let vec_store = Arc::new(Traced::new(Resilient::new(
        Timeout::new(raw.clone(), Duration::from_secs(2)),
        RetryPolicy::default(),
    )));
    let graph_store = Arc::new(Traced::new(Resilient::new(
        Timeout::new(raw.clone(), Duration::from_secs(2)),
        RetryPolicy::default(),
    )));
    let manager = Arc::new(MemoryManager::new(record_store, vec_store, graph_store));
    (raw, manager)
}

fn transform_locomo_sample(sample: &LocomoSample) -> Vec<MemoryInput> {
    let pid = ProjectId::new(sample.sample_id.clone()).unwrap();
    let mut inputs = Vec::new();

    for session_num in 1..=35 {
        let session_key = format!("session_{session_num}");
        let Some(turns_val) = sample.conversation.get(&session_key) else {
            break;
        };
        let dt_key = format!("session_{session_num}_date_time");
        let dt_str = sample
            .conversation
            .get(&dt_key)
            .and_then(|v| v.as_str())
            .unwrap_or("12:00 pm on 1 January, 2023");
        let valid_at = parse_locomo_datetime(dt_str);

        let turns: Vec<LocomoTurn> = serde_json::from_value(turns_val.clone()).unwrap();
        let session_entity = Entity {
            id: EntityId::new(format!("session:{}:{session_num}", sample.sample_id)).unwrap(),
            name: format!("Session {session_num}"),
            entity_type: "Session".into(),
        };

        for turn in turns {
            let speaker_entity = Entity {
                id: EntityId::new(format!("person:{}", slug(&turn.speaker))).unwrap(),
                name: turn.speaker.clone(),
                entity_type: "Person".into(),
            };
            let mut input = MemoryInput::new(
                pid.clone(),
                format!(
                    "[{} | {}] {}: {}",
                    session_key, dt_str, turn.speaker, turn.text
                ),
            );
            input.embedding = Some(embed_text(&turn.text));
            input.tier = MemoryTier::Episodic;
            input.scope = MemoryScope::Project;
            input.durability = WriteDurability::Buffered;
            input.idempotency_key =
                Some(format!("locomo:{}:turn:{}", sample.sample_id, turn.dia_id));
            input.entities = vec![speaker_entity.clone(), session_entity.clone()];
            input.relations = vec![Relation {
                source: speaker_entity.id.clone(),
                target: session_entity.id.clone(),
                relation_type: "SPOKE_IN".into(),
                valid_at,
                invalid_at: None,
            }];
            input.metadata = json!({
                "source_type": "locomo_turn",
                "sample_id": sample.sample_id,
                "session": session_num as i64,
                "speaker": turn.speaker,
                "dia_id": turn.dia_id,
                "timestamp_epoch": valid_at.timestamp(),
            });
            inputs.push(input);
        }

        // Transform extracted speaker observations into Semantic memories.
        let obs_key = format!("session_{session_num}_observation");
        if let Some(Value::Object(by_speaker)) = sample.observation.get(&obs_key) {
            for (speaker, items_val) in by_speaker {
                if let Some(items) = items_val.as_array() {
                    let speaker_id = EntityId::new(format!("person:{}", slug(speaker))).unwrap();
                    for (obs_idx, pair) in items.iter().enumerate() {
                        let Some(arr) = pair.as_array() else {
                            continue;
                        };
                        let Some(obs_text) = arr.first().and_then(|v| v.as_str()) else {
                            continue;
                        };
                        let dia_id = arr.get(1).and_then(|v| v.as_str()).unwrap_or("");
                        let mut input = MemoryInput::new(pid.clone(), obs_text);
                        input.embedding = Some(embed_text(obs_text));
                        input.tier = MemoryTier::Semantic;
                        input.scope = MemoryScope::Project;
                        input.durability = WriteDurability::Buffered;
                        input.idempotency_key = Some(format!(
                            "locomo:{}:obs:{session_num}:{speaker}:{obs_idx}",
                            sample.sample_id
                        ));
                        input.entities = vec![Entity {
                            id: speaker_id.clone(),
                            name: speaker.clone(),
                            entity_type: "Person".into(),
                        }];
                        input.metadata = json!({
                            "source_type": "locomo_observation",
                            "sample_id": sample.sample_id,
                            "session": session_num as i64,
                            "speaker": speaker,
                            "dia_id": dia_id,
                            "timestamp_epoch": valid_at.timestamp(),
                        });
                        inputs.push(input);
                    }
                }
            }
        }
    }

    inputs
}

#[tokio::test]
async fn e2e_locomo_transform_incorporate_and_manage() {
    let raw_json = std::fs::read_to_string(fixture_path("locomo_sample_2conv.json")).unwrap();
    let samples: Vec<LocomoSample> = serde_json::from_str(&raw_json).unwrap();
    assert_eq!(samples.len(), 2);

    let (backend, manager) = build_resilient_manager();

    // 1. Initialize vector stores for both tenants and transform + incorporate datasets.
    let mut ingested_by_dia: HashMap<(String, String), MemoryId> = HashMap::new();
    let mut total_inputs = 0usize;

    for sample in &samples {
        let pid = ProjectId::new(sample.sample_id.clone()).unwrap();
        backend
            .initialize(&pid, EMBED_DIM, DistanceMetric::Cosine)
            .await
            .unwrap();

        let transformed = transform_locomo_sample(sample);
        assert!(
            !transformed.is_empty(),
            "expected transformed records for {}",
            sample.sample_id
        );
        total_inputs += transformed.len();

        for input in transformed {
            let dia_id = input
                .metadata
                .get("dia_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let source_type = input
                .metadata
                .get("source_type")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let receipt = manager.remember(input).await.unwrap();
            if source_type == "locomo_turn" && !dia_id.is_empty() {
                ingested_by_dia.insert((sample.sample_id.clone(), dia_id), receipt.memory_id);
            }
        }
    }

    assert!(
        total_inputs > 1000,
        "expected >1000 turns+observations across 2 conversations"
    );
    manager.flush().await.unwrap();

    // 2. Verify idempotency: re-ingesting the first turn of conv-26 returns the existing MemoryId.
    let first_sample = &samples[0];
    let first_transformed = transform_locomo_sample(first_sample)
        .into_iter()
        .next()
        .unwrap();
    let first_dia = first_transformed
        .metadata
        .get("dia_id")
        .and_then(|v| v.as_str())
        .unwrap()
        .to_string();
    let dup_receipt = manager.remember(first_transformed).await.unwrap();
    assert_eq!(
        dup_receipt.memory_id,
        ingested_by_dia[&(first_sample.sample_id.clone(), first_dia)]
    );

    // 3. Manage & Recall: run single-hop QA probes from conv-26 with FilterExpr + Hybrid mode.
    let conv26_pid = ProjectId::new("conv-26").unwrap();
    let single_hop_qas: Vec<&LocomoQa> = first_sample
        .qa
        .iter()
        .filter(|q| q.category == 4 && !q.evidence.is_empty())
        .take(5)
        .collect();
    assert_eq!(single_hop_qas.len(), 5);

    let mut top10_evidence_hits = 0usize;
    for qa in &single_hop_qas {
        let mut query = RecallQuery::new(conv26_pid.clone(), qa.question.clone());
        query.query_vector = Some(embed_text(&qa.question));
        query.seed_entities = vec![EntityId::new("person:caroline").unwrap()];
        query.mode = RecallMode::Hybrid;
        query.top_k = 20;
        query.consistency = ConsistencyMode::ReadYourWrites;
        query.filters = Some(FilterExpr::Eq {
            field: "sample_id".into(),
            value: FilterValue::String("conv-26".into()),
        });

        let result = manager.recall(query).await.unwrap();
        assert!(!result.hits.is_empty());
        assert!(
            result
                .hits
                .iter()
                .all(|h| h.metadata.get("sample_id").and_then(|v| v.as_str()) == Some("conv-26")),
            "cross-tenant leakage detected"
        );
        assert!(
            !result.subgraph.edges.is_empty(),
            "expected graph neighborhood for person:caroline"
        );

        let expected_ids: Vec<MemoryId> = qa
            .evidence
            .iter()
            .filter_map(|ev| {
                ingested_by_dia
                    .get(&("conv-26".into(), ev.clone()))
                    .copied()
            })
            .collect();
        if result
            .hits
            .iter()
            .any(|h| expected_ids.contains(&h.memory_id))
        {
            top10_evidence_hits += 1;
        }
    }
    assert!(
        top10_evidence_hits >= 1,
        "expected deterministic embedding recall to surface at least one ground-truth evidence turn"
    );

    // 4. Range + In FilterExpr test: only Caroline's turns in sessions 1..=3.
    let mut filtered_query = RecallQuery::new(conv26_pid.clone(), "LGBTQ support group");
    filtered_query.query_vector = Some(embed_text("LGBTQ support group"));
    filtered_query.mode = RecallMode::Vector;
    filtered_query.top_k = 10;
    filtered_query.filters = Some(FilterExpr::And(vec![
        FilterExpr::Eq {
            field: "speaker".into(),
            value: FilterValue::String("Caroline".into()),
        },
        FilterExpr::Range {
            field: "session".into(),
            gte: Some(1.0),
            lte: Some(3.0),
        },
    ]));
    let filtered_res = manager.recall(filtered_query).await.unwrap();
    assert!(!filtered_res.hits.is_empty());
    for hit in &filtered_res.hits {
        assert_eq!(hit.metadata["speaker"], "Caroline");
        let s = hit.metadata["session"].as_i64().unwrap();
        assert!((1..=3).contains(&s));
    }

    manager.shutdown().await.unwrap();
}

#[tokio::test]
async fn e2e_knowledge_lifecycle_cas_temporal_and_forget() {
    let raw_json = std::fs::read_to_string(fixture_path("knowledge_lifecycle.json")).unwrap();
    let fixture: KnowledgeLifecycleFixture = serde_json::from_str(&raw_json).unwrap();
    let (backend, manager) = build_resilient_manager();

    for proj in &fixture.projects {
        let pid = ProjectId::new(proj.project_id.clone()).unwrap();
        backend
            .initialize(&pid, EMBED_DIM, DistanceMetric::Cosine)
            .await
            .unwrap();

        let mut id_map: HashMap<String, MemoryId> = HashMap::new();
        for rec in &proj.records {
            let mut input = MemoryInput::new(pid.clone(), rec.content.clone());
            input.embedding = Some(embed_text(&rec.content));
            input.tier = rec.tier;
            input.scope = rec.scope;
            input.durability = WriteDurability::WaitFlush;
            input.idempotency_key = Some(rec.idempotency_key.clone());
            input.entities = rec.entities.clone();
            input.relations = rec.relations.clone();
            input.metadata = rec.metadata.clone();

            let receipt = manager.remember(input).await.unwrap();
            assert_eq!(receipt.state, ReceiptState::Persisted);
            assert_eq!(receipt.version, 1);
            id_map.insert(rec.id.clone(), receipt.memory_id);
        }

        // Temporal graph query: as_of 2026-01-20 should include Jan 15 edges, exclude Feb 1 edge.
        let as_of_jan = DateTime::parse_from_rfc3339("2026-01-20T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let mut temporal_query = RecallQuery::new(pid.clone(), "Team Alpha databases");
        temporal_query.mode = RecallMode::Graph;
        temporal_query.seed_entities = vec![EntityId::new("team:alpha").unwrap()];
        temporal_query.temporal = TemporalFilter {
            as_of: Some(as_of_jan),
            active_only: true,
        };
        let jan_graph = manager.recall(temporal_query).await.unwrap();
        assert_eq!(jan_graph.subgraph.edges.len(), 2);

        // CAS update + stale conflict check.
        for upd in &proj.updates {
            let mem_id = id_map[&upd.target_id];
            let updated = manager
                .update(
                    &pid,
                    mem_id,
                    MemoryUpdate {
                        content: Some(upd.new_content.clone()),
                        embedding: Some(embed_text(&upd.new_content)),
                        metadata: Some(upd.new_metadata.clone()),
                        entities: None,
                        relations: None,
                        expected_version: Some(1),
                    },
                )
                .await
                .unwrap();
            assert_eq!(updated.version, 2);

            let conflict = manager
                .update(
                    &pid,
                    mem_id,
                    MemoryUpdate {
                        content: Some("stale overwrite".into()),
                        embedding: None,
                        metadata: None,
                        entities: None,
                        relations: None,
                        expected_version: Some(1),
                    },
                )
                .await;
            assert!(matches!(conflict, Err(AetherError::Conflict { .. })));
        }

        // Forget target record and verify it is absent from VectorStore.
        for target in &proj.forget_targets {
            let mem_id = id_map[target];
            manager.forget(&pid, mem_id).await.unwrap();

            let mut q = RecallQuery::new(pid.clone(), "Qdrant staging node restarted");
            q.query_vector = Some(embed_text("Qdrant staging node restarted"));
            q.mode = RecallMode::Vector;
            q.top_k = 10;
            let res = manager.recall(q).await.unwrap();
            assert!(res.hits.iter().all(|h| h.memory_id != mem_id));
        }
    }

    manager.shutdown().await.unwrap();
}
