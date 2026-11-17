mod common;

use std::collections::HashMap;

use aether_core::{
    AetherError, ConsistencyMode, DistanceMetric, Entity, EntityId, FilterExpr, FilterValue,
    MemoryId, MemoryInbound, MemoryInput, MemoryScope, MemoryTier, MemoryUpdate, ProjectId,
    RecallMode, RecallQuery, ReceiptState, Relation, TemporalFilter, VectorStore, WriteDurability,
};
use chrono::{DateTime, Utc};
use common::{
    build_resilient_manager, embed_text, fixture_path, transform_locomo_sample, LocomoQa,
    LocomoSample, EMBED_DIM,
};
use serde::Deserialize;
use serde_json::Value;

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

#[tokio::test]
async fn e2e_boundary_validation_rejects_invalid_inputs() {
    let (backend, manager) = build_resilient_manager();
    let pid = ProjectId::new("boundary-test").unwrap();
    backend
        .initialize(&pid, EMBED_DIM, DistanceMetric::Cosine)
        .await
        .unwrap();

    // Reject blank ProjectId at the type boundary.
    assert!(matches!(
        ProjectId::new(""),
        Err(AetherError::InvalidInput(_))
    ));
    assert!(matches!(
        ProjectId::new("   \t\n "),
        Err(AetherError::InvalidInput(_))
    ));

    // Reject empty or whitespace-only content on remember().
    let empty_err = manager
        .remember(MemoryInput::new(pid.clone(), ""))
        .await
        .unwrap_err();
    assert!(matches!(empty_err, AetherError::InvalidInput(_)));

    let ws_err = manager
        .remember(MemoryInput::new(pid.clone(), "   \n "))
        .await
        .unwrap_err();
    assert!(matches!(ws_err, AetherError::InvalidInput(_)));

    // Reject non-finite NaN embedding on remember().
    let mut bad_vec = vec![0.0f32; EMBED_DIM];
    bad_vec[0] = f32::NAN;
    let mut nan_input = MemoryInput::new(pid.clone(), "valid content with NaN vector");
    nan_input.embedding = Some(bad_vec);
    let nan_err = manager.remember(nan_input).await.unwrap_err();
    assert!(matches!(nan_err, AetherError::InvalidInput(_)));

    manager.shutdown().await.unwrap();
}
