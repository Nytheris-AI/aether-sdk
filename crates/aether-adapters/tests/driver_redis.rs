#![cfg(feature = "redis")]

use std::io;
use std::time::Duration;

use aether_adapters::kv::redis::{map_redis_error, RedisConfig, RedisRecordStore};
use aether_core::{
    AetherError, ConnectorHealth, MemoryId, MemoryInput, MemoryItem, MemoryReceipt, MemoryScope,
    OutboundConnector, ProjectId, ReceiptState, RecordStore,
};
use redis::{ErrorKind, RedisError};
use uuid::Uuid;

#[test]
fn config_debug_redacts_credentials_and_preserves_host() {
    let with_creds = RedisConfig::new("redis://alice:s3cr3t@cache.internal:6379/0", "aether-prod");
    let rendered = format!("{with_creds:?}");
    assert!(!rendered.contains("s3cr3t"), "leaked password: {rendered}");
    assert!(!rendered.contains("alice"), "leaked username: {rendered}");
    assert!(
        rendered.contains("redis://[REDACTED]@cache.internal:6379/0"),
        "expected redacted authority in {rendered}"
    );
    assert!(rendered.contains("aether-prod"));

    let no_creds = RedisConfig::new("redis://127.0.0.1:6379", "aether-dev");
    let rendered_plain = format!("{no_creds:?}");
    assert!(rendered_plain.contains("redis://127.0.0.1:6379"));
    assert!(rendered_plain.contains("aether-dev"));
}

#[test]
fn config_validation_rejects_empty_or_malformed_inputs() {
    let empty_url = RedisConfig::new("   ", "aether");
    assert!(matches!(
        empty_url.validate(),
        Err(AetherError::InvalidInput(_))
    ));

    let empty_prefix = RedisConfig::new("redis://127.0.0.1:6379", "  ");
    assert!(matches!(
        empty_prefix.validate(),
        Err(AetherError::InvalidInput(_))
    ));

    let bad_url = RedisConfig::new("not-a-valid-redis-url", "aether");
    assert!(matches!(
        bad_url.validate(),
        Err(AetherError::InvalidInput(_))
    ));

    let valid = RedisConfig::new("redis://:pass@127.0.0.1:6379/0", "aether");
    assert!(valid.validate().is_ok());
}

#[tokio::test]
async fn connect_rejects_invalid_config_without_network_io() {
    let err = RedisRecordStore::connect(RedisConfig::new("not-a-url", "aether"))
        .await
        .unwrap_err();
    assert!(matches!(err, AetherError::InvalidInput(_)));
}

#[test]
fn key_layout_scopes_by_prefix_and_project_and_normalizes_trailing_colons() {
    let cfg_plain = RedisConfig::new("redis://127.0.0.1:6379", "aether");
    let cfg_colon = RedisConfig::new("redis://127.0.0.1:6379", "aether:");

    let layout_plain = cfg_plain.key_layout().unwrap();
    let layout_colon = cfg_colon.key_layout().unwrap();

    let ns_a = ProjectId::new("proj-alpha").unwrap();
    let ns_b = ProjectId::new("proj-beta").unwrap();
    let mem_id = MemoryId::new();

    assert_eq!(
        layout_plain.record_key(&ns_a, mem_id),
        layout_colon.record_key(&ns_a, mem_id)
    );
    assert_eq!(
        layout_plain.idempotency_key(&ns_a, "idem-1"),
        layout_colon.idempotency_key(&ns_a, "idem-1")
    );
    assert_eq!(
        layout_plain.recent_index_key(&ns_a),
        layout_colon.recent_index_key(&ns_a)
    );

    let rec_a = layout_plain.record_key(&ns_a, mem_id);
    let rec_b = layout_plain.record_key(&ns_b, mem_id);
    assert_ne!(rec_a, rec_b);
    assert!(rec_a.contains("aether"));
    assert!(rec_a.contains("proj-alpha"));
    assert!(rec_a.contains(&mem_id.to_string()));

    let idem_a = layout_plain.idempotency_key(&ns_a, "idem-1");
    let idem_b = layout_plain.idempotency_key(&ns_b, "idem-1");
    assert_ne!(idem_a, idem_b);
    assert_ne!(idem_a, rec_a);

    let recent_a = layout_plain.recent_index_key(&ns_a);
    let recent_b = layout_plain.recent_index_key(&ns_b);
    assert_ne!(recent_a, recent_b);
    assert_ne!(recent_a, rec_a);
    assert_ne!(recent_a, idem_a);
}

#[test]
fn error_mapping_classifies_transient_and_permanent_redis_errors() {
    let transient_cases = [
        RedisError::from(io::Error::new(io::ErrorKind::TimedOut, "read timed out")),
        RedisError::from(io::Error::new(
            io::ErrorKind::ConnectionRefused,
            "connection refused",
        )),
        RedisError::from(io::Error::new(io::ErrorKind::BrokenPipe, "broken pipe")),
        RedisError::from((ErrorKind::BusyLoadingError, "loading dataset")),
        RedisError::from((ErrorKind::TryAgain, "try again")),
        RedisError::from((ErrorKind::ClusterDown, "cluster down")),
        RedisError::from((ErrorKind::MasterDown, "master down")),
    ];

    for err in transient_cases {
        let mapped = map_redis_error(err);
        assert!(
            matches!(
                mapped,
                AetherError::Transient {
                    connector: "redis",
                    ..
                }
            ),
            "expected Transient, got {mapped:?}"
        );
        assert!(mapped.is_retryable());
    }

    let permanent_cases = [
        RedisError::from((ErrorKind::AuthenticationFailed, "invalid password")),
        RedisError::from((ErrorKind::InvalidClientConfig, "invalid client url")),
        RedisError::from((ErrorKind::TypeError, "WRONGTYPE operation")),
        RedisError::from((ErrorKind::ResponseError, "ERR syntax error")),
        RedisError::from((ErrorKind::ParseError, "unexpected protocol byte")),
    ];

    for err in permanent_cases {
        let mapped = map_redis_error(err);
        assert!(
            matches!(
                mapped,
                AetherError::Permanent {
                    connector: "redis",
                    ..
                }
            ),
            "expected Permanent, got {mapped:?}"
        );
        assert!(!mapped.is_retryable());
    }
}

#[cfg(feature = "contract-suite")]
#[tokio::test]
#[ignore = "requires live Redis/Valkey server (default redis://127.0.0.1:56379)"]
async fn live_redis_contract_record_store() {
    let url = std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:56379".to_string());
    let prefix = format!("aether-contract-{}", Uuid::new_v4());
    let store = RedisRecordStore::connect(RedisConfig::new(url, prefix))
        .await
        .expect("connect to live Redis/Valkey");

    assert_eq!(store.name(), "redis");
    assert_eq!(
        store.health_check().await.unwrap(),
        ConnectorHealth::Healthy
    );
    assert!(!format!("{store:?}").contains("s3cr3t"));

    aether_adapters::contract::record_store(&store).await;

    // Additional edge-case checks: empty recent, single-item recent (1-key MGET),
    // idempotency TTL expiration, CAS NotFound/Conflict fields/expected_version=None,
    // rapid recent ordering, TTL lazy pruning, and scope filtering beyond top-N.
    let empty_ns = ProjectId::new(format!("empty-{}", Uuid::new_v4())).unwrap();
    assert!(store.recent(&empty_ns, None, 10).await.unwrap().is_empty());

    let ns = ProjectId::new(format!("edge-{}", Uuid::new_v4())).unwrap();
    let missing_id = MemoryId::new();
    let missing_item = MemoryItem::from_input(MemoryInput::new(ns.clone(), "missing"), 1);
    let missing_res = store
        .cas_update(&ns, missing_id, Some(1), &missing_item)
        .await;
    assert!(matches!(missing_res, Err(AetherError::NotFound(_))));

    let receipt = MemoryReceipt {
        memory_id: MemoryId::new(),
        state: ReceiptState::Buffered,
        version: 1,
    };
    assert!(store
        .put_if_absent(&ns, "idem-ttl", &receipt, Some(Duration::from_millis(25)))
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        store
            .put_if_absent(&ns, "idem-ttl", &receipt, Some(Duration::from_millis(25)))
            .await
            .unwrap(),
        Some(receipt.clone())
    );
    tokio::time::sleep(Duration::from_millis(40)).await;
    assert!(store
        .put_if_absent(&ns, "idem-ttl", &receipt, None)
        .await
        .unwrap()
        .is_none());

    let mut session_input = MemoryInput::new(ns.clone(), "older session item");
    session_input.scope = MemoryScope::Session;
    let session_item = MemoryItem::from_input(session_input, 1);
    store.put(&ns, &session_item, None).await.unwrap();
    let single_recent = store.recent(&ns, None, 10).await.unwrap();
    assert_eq!(single_recent.len(), 1);
    assert_eq!(single_recent[0].id, session_item.id);

    let mut expiring_input = MemoryInput::new(ns.clone(), "short-lived item");
    expiring_input.scope = MemoryScope::Project;
    let expiring_item = MemoryItem::from_input(expiring_input, 1);
    store
        .put(&ns, &expiring_item, Some(Duration::from_millis(25)))
        .await
        .unwrap();

    let mut newer_proj_input = MemoryInput::new(ns.clone(), "newer project item");
    newer_proj_input.scope = MemoryScope::Project;
    let newer_proj_item = MemoryItem::from_input(newer_proj_input, 1);
    store.put(&ns, &newer_proj_item, None).await.unwrap();

    tokio::time::sleep(Duration::from_millis(45)).await;

    let session_only = store
        .recent(&ns, Some(MemoryScope::Session), 1)
        .await
        .unwrap();
    assert_eq!(session_only.len(), 1);
    assert_eq!(session_only[0].id, session_item.id);

    let all_items = store.recent(&ns, None, 10).await.unwrap();
    assert_eq!(all_items.len(), 2);
    assert_eq!(all_items[0].id, newer_proj_item.id);
    assert_eq!(all_items[1].id, session_item.id);

    // CAS update with expected_version = None succeeds on existing key and preserves version update.
    let mut updated = session_item.clone();
    updated.content = "updated without expected version".into();
    updated.version = 2;
    store
        .cas_update(&ns, session_item.id, None, &updated)
        .await
        .unwrap();
    let fetched = store.get(&ns, session_item.id).await.unwrap().unwrap();
    assert_eq!(fetched.version, 2);
    assert_eq!(fetched.content, "updated without expected version");

    let conflict = store
        .cas_update(&ns, session_item.id, Some(1), &updated)
        .await;
    assert!(matches!(
        conflict,
        Err(AetherError::Conflict {
            memory_id,
            expected_version: 1,
            actual_version: 2,
        }) if memory_id == session_item.id
    ));

    store.close().await.unwrap();
}
