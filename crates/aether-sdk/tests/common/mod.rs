#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use nytheris_aether_adapters::{InMemoryBackend, RetryPolicy};
use nytheris_aether_core::{
    Entity, EntityId, MemoryInput, MemoryScope, MemoryTier, ProjectId, Relation, WriteDurability,
};
use nytheris_aether_sdk::MemoryManager;
use serde::Deserialize;
use serde_json::{json, Value};

pub const EMBED_DIM: usize = 64;

/// Deterministic feature-hashed token + bigram embedding normalized to unit L2 norm.
pub fn embed_text(text: &str) -> Vec<f32> {
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

pub fn fnv1a_hash(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

pub fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

pub fn build_fault_injectable_manager(raw: Arc<InMemoryBackend>) -> Arc<MemoryManager> {
    Arc::new(
        MemoryManager::builder(raw.clone())
            .vector(raw.clone())
            .graph(raw)
            .with_timeout(Some(Duration::from_secs(2)))
            .with_retry(Some(RetryPolicy::default()))
            .build(),
    )
}

pub fn build_resilient_manager() -> (Arc<InMemoryBackend>, Arc<MemoryManager>) {
    let raw = Arc::new(InMemoryBackend::new());
    let manager = build_fault_injectable_manager(raw.clone());
    (raw, manager)
}

pub fn slug(raw: &str) -> String {
    raw.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect()
}

/// Parses LoCoMo timestamps like `"1:56 pm on 8 May, 2023"` into UTC `DateTime`.
pub fn parse_locomo_datetime(raw: &str) -> DateTime<Utc> {
    if let Ok(naive) = NaiveDateTime::parse_from_str(raw.trim(), "%-I:%M %P on %-d %B, %Y") {
        return Utc.from_utc_datetime(&naive);
    }
    Utc.with_ymd_and_hms(2023, 1, 1, 0, 0, 0).unwrap()
}

#[derive(Debug, Deserialize)]
pub struct LocomoSample {
    pub sample_id: String,
    pub conversation: BTreeMap<String, Value>,
    #[serde(default)]
    pub observation: BTreeMap<String, Value>,
    pub qa: Vec<LocomoQa>,
}

#[derive(Debug, Deserialize)]
pub struct LocomoTurn {
    pub speaker: String,
    pub dia_id: String,
    pub text: String,
}

#[derive(Debug, Deserialize)]
pub struct LocomoQa {
    pub question: String,
    pub evidence: Vec<String>,
    pub category: u8,
}

pub fn transform_locomo_sample(sample: &LocomoSample) -> Vec<MemoryInput> {
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
