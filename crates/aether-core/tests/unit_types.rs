use std::sync::Arc;

use aether_core::{
    AetherError, ConnectorCapabilities, ConnectorHealth, ConsistencyMode, DistanceMetric, Entity,
    EntityId, FilterExpr, FilterValue, GraphStore, MemoryId, MemoryInbound, MemoryInput,
    MemoryItem, MemoryReceipt, MemoryScope, MemoryTier, OutboundConnector, ProjectId, RecallMode,
    RecallQuery, ReceiptState, RecordStore, Relation, TemporalFilter, VectorRecord, VectorStore,
    WriteDurability,
};
use chrono::Utc;
use uuid::Uuid;

#[test]
fn newtype_ids_validate_and_roundtrip() {
    assert!(ProjectId::new("").is_err());
    assert!(ProjectId::new("   ").is_err());
    assert!(EntityId::new("").is_err());

    let pid = ProjectId::new("proj-alpha").unwrap();
    let mid = MemoryId::new();
    let eid = EntityId::new("person:alice").unwrap();

    let pid_json = serde_json::to_string(&pid).unwrap();
    let mid_json = serde_json::to_string(&mid).unwrap();
    let eid_json = serde_json::to_string(&eid).unwrap();

    assert_eq!(serde_json::from_str::<ProjectId>(&pid_json).unwrap(), pid);
    assert_eq!(serde_json::from_str::<MemoryId>(&mid_json).unwrap(), mid);
    assert_eq!(serde_json::from_str::<EntityId>(&eid_json).unwrap(), eid);
}

#[test]
fn error_retryable_classification_distinguishes_transient_and_permanent() {
    let transient = AetherError::Transient {
        connector: "qdrant",
        message: "429 too many requests".into(),
    };
    let permanent = AetherError::Permanent {
        connector: "qdrant",
        message: "401 unauthorized".into(),
    };
    let invalid = AetherError::InvalidInput("empty content".into());
    let conflict = AetherError::Conflict {
        memory_id: MemoryId::new(),
        expected_version: 2,
        actual_version: 3,
    };

    assert!(transient.is_retryable());
    assert!(!permanent.is_retryable());
    assert!(!invalid.is_retryable());
    assert!(!conflict.is_retryable());
}

#[test]
fn filter_expr_and_temporal_filter_roundtrip_serde() {
    let filter = FilterExpr::And(vec![
        FilterExpr::Eq {
            field: "tier".into(),
            value: FilterValue::String("semantic".into()),
        },
        FilterExpr::In {
            field: "tag".into(),
            values: vec![
                FilterValue::String("rust".into()),
                FilterValue::String("sdk".into()),
            ],
        },
        FilterExpr::Range {
            field: "confidence".into(),
            gte: Some(0.5),
            lte: Some(1.0),
        },
    ]);

    let json = serde_json::to_string(&filter).unwrap();
    assert_eq!(serde_json::from_str::<FilterExpr>(&json).unwrap(), filter);

    let temporal = TemporalFilter::default();
    assert!(temporal.active_only);
    assert!(temporal.as_of.is_none());

    let t_json = serde_json::to_string(&temporal).unwrap();
    assert_eq!(
        serde_json::from_str::<TemporalFilter>(&t_json).unwrap(),
        temporal
    );
}

#[test]
fn memory_input_validation_and_receipt_states() {
    let mut input = MemoryInput::new(
        ProjectId::new("proj-1").unwrap(),
        "Alice prefers Turbopuffer for low-latency vector recall.",
    );
    input.tier = MemoryTier::Semantic;
    input.scope = MemoryScope::User;
    input.durability = WriteDurability::WaitFlush;
    input.embedding = Some(vec![0.1, 0.2, 0.3]);
    input.entities = vec![Entity {
        id: EntityId::new("person:alice").unwrap(),
        name: "Alice".into(),
        entity_type: "Person".into(),
    }];
    input.relations = vec![Relation {
        source: EntityId::new("person:alice").unwrap(),
        target: EntityId::new("db:turbopuffer").unwrap(),
        relation_type: "PREFERS".into(),
        valid_at: Utc::now(),
        invalid_at: None,
    }];

    assert!(input.validate().is_ok());

    let mut bad_input = input.clone();
    bad_input.content = "   ".into();
    assert!(bad_input.validate().is_err());

    let mut bad_vec = input.clone();
    bad_vec.embedding = Some(vec![]);
    assert!(bad_vec.validate().is_err());

    let receipt = MemoryReceipt {
        memory_id: MemoryId::from_uuid(Uuid::new_v4()),
        state: ReceiptState::Buffered,
        version: 1,
    };
    let r_json = serde_json::to_string(&receipt).unwrap();
    assert_eq!(
        serde_json::from_str::<MemoryReceipt>(&r_json).unwrap(),
        receipt
    );
}

#[test]
fn recall_query_and_connector_types_roundtrip_and_traits_are_object_safe() {
    let query = RecallQuery {
        project_id: ProjectId::new("proj-1").unwrap(),
        query: "What vector DB does Alice prefer?".into(),
        query_vector: Some(vec![0.1, 0.2, 0.3]),
        seed_entities: vec![EntityId::new("person:alice").unwrap()],
        mode: RecallMode::Hybrid,
        top_k: 5,
        hops: 2,
        per_node_limit: 20,
        temporal: TemporalFilter::default(),
        filters: None,
        consistency: ConsistencyMode::ReadYourWrites,
    };
    let q_json = serde_json::to_string(&query).unwrap();
    assert_eq!(serde_json::from_str::<RecallQuery>(&q_json).unwrap(), query);

    let item = MemoryItem::from_input(input_fixture(), 1);
    assert_eq!(item.version, 1);

    let caps = ConnectorCapabilities {
        max_batch_size: 100,
        supports_temporal: true,
        supports_filter_expr: true,
    };
    assert_eq!(caps.max_batch_size, 100);
    let _health = ConnectorHealth::Healthy;
    let _metric = DistanceMetric::Cosine;
    let _rec = VectorRecord {
        id: item.id,
        vector: vec![0.1, 0.2, 0.3],
        metadata: serde_json::json!({}),
    };

    fn _assert_object_safe(
        _inbound: Arc<dyn MemoryInbound>,
        _conn: Arc<dyn OutboundConnector>,
        _records: Arc<dyn RecordStore>,
        _vec: Arc<dyn VectorStore>,
        _grp: Arc<dyn GraphStore>,
    ) {
    }
}

fn input_fixture() -> MemoryInput {
    MemoryInput::new(ProjectId::new("proj-1").unwrap(), "Fact")
}

#[test]
fn distance_metric_score_is_higher_for_closer_vectors() {
    let q = [1.0, 0.0];
    for m in [
        DistanceMetric::Cosine,
        DistanceMetric::Dot,
        DistanceMetric::Euclidean,
    ] {
        assert!(m.score(&q, &[1.0, 0.0]) > m.score(&q, &[0.0, 1.0]), "{m:?}");
    }
    assert_eq!(DistanceMetric::Cosine.score(&q, &[0.0, 0.0]), 0.0);
}

#[test]
fn filter_expr_matches_metadata() {
    let meta = serde_json::json!({"tier": "semantic", "n": 3});
    let eq = FilterExpr::Eq {
        field: "tier".into(),
        value: FilterValue::String("semantic".into()),
    };
    let range = FilterExpr::Range {
        field: "n".into(),
        gte: Some(4.0),
        lte: None,
    };
    assert!(eq.matches(&meta));
    assert!(!range.matches(&meta));
    assert!(FilterExpr::Or(vec![eq.clone(), range.clone()]).matches(&meta));
    assert!(!FilterExpr::And(vec![eq, range]).matches(&meta));
}
