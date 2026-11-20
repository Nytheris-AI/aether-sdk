use std::sync::Arc;

use chrono::Utc;
use muninn_aether_adapters::InMemoryBackend;
use muninn_aether_core::{
    DistanceMetric, Entity, EntityId, MemoryInput, MemoryUpdate, ProjectId, RecallMode,
    RecallQuery, Relation, VectorStore, WriteDurability,
};
use muninn_aether_sdk::telemetry::{init_otel, LogFormat, OtelConfig};
use muninn_aether_sdk::MemoryManager;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. One-line OTel setup (OTLP traces + OTLP logs + stdout logs)
    let mut telemetry =
        init_otel(OtelConfig::from_env("aether-sdk-demo").with_log_format(LogFormat::Pretty))?;

    // 2. Zero-boilerplate MemoryManager (auto-wraps stores with Traced<T>)
    let backend = Arc::new(InMemoryBackend::new());
    let pid = ProjectId::new("demo-project")?;
    backend.initialize(&pid, 3, DistanceMetric::Cosine).await?;

    let manager = MemoryManager::builder(backend.clone())
        .vector(backend.clone())
        .graph(backend)
        .build();

    // 3. Remember (WaitFlush -> exercises RecordStore + Indexer + VectorStore + GraphStore)
    let mut input = MemoryInput::new(pid.clone(), "Alice uses Aether SDK with OpenTelemetry.");
    input.embedding = Some(vec![1.0, 0.0, 0.0]);
    input.durability = WriteDurability::WaitFlush;
    input.entities = vec![Entity {
        id: EntityId::new("person:alice")?,
        name: "Alice".into(),
        entity_type: "Person".into(),
    }];
    input.relations = vec![Relation {
        source: EntityId::new("person:alice")?,
        target: EntityId::new("tool:otel")?,
        relation_type: "OBSERVES_WITH".into(),
        valid_at: Utc::now(),
        invalid_at: None,
    }];
    let receipt = manager.remember(input).await?;
    tracing::info!(memory_id = %receipt.memory_id, "stored memory");

    // 4. Hybrid Recall
    let mut query = RecallQuery::new(pid.clone(), "Alice OpenTelemetry");
    query.query_vector = Some(vec![1.0, 0.0, 0.0]);
    query.seed_entities = vec![EntityId::new("person:alice")?];
    query.mode = RecallMode::Hybrid;
    let recalled = manager.recall(query).await?;
    tracing::info!(hits = recalled.hits.len(), "recalled memories");

    // 5. Trigger an expected Conflict error to verify otel.status_code = "ERROR"
    let err = manager
        .update(
            &pid,
            receipt.memory_id,
            MemoryUpdate {
                content: Some("stale".into()),
                embedding: None,
                metadata: None,
                entities: None,
                relations: None,
                expected_version: Some(999),
            },
        )
        .await;
    tracing::info!(is_err = err.is_err(), "verified error span recording");

    manager.shutdown().await?;
    telemetry.shutdown();
    Ok(())
}
