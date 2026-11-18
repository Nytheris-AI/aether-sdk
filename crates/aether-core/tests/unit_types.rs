use std::sync::Arc;

use chrono::Utc;
use nytheris_aether_core::{
    redact_url_userinfo, AetherError, ConnectorCapabilities, ConnectorHealth, ConsistencyMode,
    DistanceMetric, Entity, EntityId, FilterExpr, FilterValue, GraphStore, IndexSpec, MemoryId,
    MemoryInput, MemoryItem, MemoryReceipt, MemoryScope, MemoryTier, MemoryUpdate,
    OutboundConnector, ProjectId, RecallMode, RecallQuery, ReceiptState, RecordStore, Relation,
    SecretString, TemporalFilter, VectorRecord, VectorStore, WriteDurability,
};
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
    input.scope = MemoryScope::User(nytheris_aether_core::UserId::new("u-1").unwrap());
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
        scopes: vec![MemoryScope::Project],
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

#[test]
fn secret_string_redacts_debug_and_display_and_exposes_explicitly() {
    let secret = SecretString::new("my-secret-token");
    assert_eq!(format!("{secret:?}"), "[REDACTED]");
    assert_eq!(format!("{secret}"), "[REDACTED]");
    assert!(!format!("{secret:?}").contains("my-secret-token"));
    assert_eq!(secret.expose_secret(), "my-secret-token");
}

#[test]
fn redact_url_userinfo_strips_credentials_from_authority_only() {
    assert_eq!(
        redact_url_userinfo("https://alice:dbpass123@otel.example.com:4317/v1/traces"),
        "https://[REDACTED]@otel.example.com:4317/v1/traces"
    );
    assert_eq!(
        redact_url_userinfo("redis://:cachepass@localhost:6379/0"),
        "redis://[REDACTED]@localhost:6379/0"
    );
    assert_eq!(
        redact_url_userinfo("http://localhost:4317/v1?user=a@b.com"),
        "http://localhost:4317/v1?user=a@b.com"
    );
    assert_eq!(redact_url_userinfo("not-a-url"), "not-a-url");
}

#[test]
fn memory_input_rejects_non_object_and_reserved_metadata() {
    let mut bad_type = input_fixture();
    bad_type.metadata = serde_json::json!(["not", "an", "object"]);
    assert!(matches!(
        bad_type.validate(),
        Err(AetherError::InvalidInput(_))
    ));

    let mut reserved = input_fixture();
    reserved.metadata = serde_json::json!({"_aether_scope": "project"});
    assert!(matches!(
        reserved.validate(),
        Err(AetherError::InvalidInput(_))
    ));
}

#[test]
fn memory_update_rejects_invalid_metadata_and_non_finite_embedding() {
    let mut bad_meta = MemoryUpdate {
        content: None,
        embedding: None,
        metadata: Some(serde_json::json!({"_aether_tier": "episodic"})),
        entities: None,
        relations: None,
        expected_version: None,
    };
    assert!(matches!(
        bad_meta.validate(),
        Err(AetherError::InvalidInput(_))
    ));

    bad_meta.metadata = None;
    bad_meta.embedding = Some(vec![1.0, f32::NAN]);
    assert!(matches!(
        bad_meta.validate(),
        Err(AetherError::InvalidInput(_))
    ));
}

#[test]
fn index_spec_check_validates_dimensions_and_finiteness() {
    let spec = IndexSpec {
        dimensions: 3,
        metric: DistanceMetric::Cosine,
    };
    assert!(spec.check(&[1.0, 0.0, 0.0]).is_ok());
    assert!(matches!(
        spec.check(&[1.0, 0.0]),
        Err(AetherError::InvalidInput(_))
    ));
    assert!(matches!(
        spec.check(&[1.0, f32::INFINITY, 0.0]),
        Err(AetherError::InvalidInput(_))
    ));
}

#[test]
fn recall_query_validation() {
    let pid = ProjectId::new("proj-q").unwrap();
    let missing_vec = RecallQuery::new(pid.clone(), "hello");
    assert!(matches!(
        missing_vec.validate(),
        Err(AetherError::InvalidInput(_))
    ));

    let mut graph_no_seeds = RecallQuery::new(pid.clone(), "hello");
    graph_no_seeds.mode = RecallMode::Graph;
    assert!(matches!(
        graph_no_seeds.validate(),
        Err(AetherError::InvalidInput(_))
    ));

    let mut nan_vec = RecallQuery::new(pid, "hello");
    nan_vec.mode = RecallMode::Vector;
    nan_vec.query_vector = Some(vec![1.0, f32::NAN]);
    assert!(matches!(
        nan_vec.validate(),
        Err(AetherError::InvalidInput(_))
    ));
}

#[test]
fn memory_scope_serde_and_index_key() {
    assert!(nytheris_aether_core::UserId::new(" ").is_err());
    assert!(nytheris_aether_core::SessionId::new("").is_err());

    let user_scope = MemoryScope::User(nytheris_aether_core::UserId::new("u1").unwrap());
    let user_json = serde_json::to_value(&user_scope).unwrap();
    assert_eq!(user_json, serde_json::json!({"kind": "user", "id": "u1"}));
    assert_eq!(user_scope.index_key().as_deref(), Some("user:u1"));

    let proj_json = serde_json::to_value(MemoryScope::Project).unwrap();
    assert_eq!(proj_json, serde_json::json!({"kind": "project"}));
    assert_eq!(MemoryScope::Project.index_key().as_deref(), Some("project"));

    let sess_scope = MemoryScope::Session(nytheris_aether_core::SessionId::new("s1").unwrap());
    assert_eq!(sess_scope.index_key(), None);

    let mut q = RecallQuery::new(ProjectId::new("proj-scope").unwrap(), "q");
    q.mode = RecallMode::Vector;
    q.query_vector = Some(vec![1.0]);
    q.scopes = vec![];
    assert!(matches!(q.validate(), Err(AetherError::InvalidInput(_))));
}
