#![cfg(feature = "redis")]

use std::io;
use std::sync::Arc;
use std::time::Duration;

use muninn_aether_adapters::kv::redis::{map_redis_error, RedisConfig, RedisRecordStore};
use muninn_aether_core::{
    AetherError, ConnectorHealth, CreateOutcome, MemoryId, MemoryInput, MemoryItem, MemoryScope,
    OutboundConnector, ProjectId, ReceiptState, RecordStore, SessionId,
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

    let sess = SessionId::new("sess-1").unwrap();
    let pend_a = layout_plain.project_pending_key(&ns_a);
    let pend_b = layout_plain.project_pending_key(&ns_b);
    let g_pend = layout_plain.global_pending_key();
    let sess_a = layout_plain.session_index_key(&ns_a, &sess);
    let rec_idem_a = layout_plain.record_idem_key(&ns_a, mem_id);
    assert_ne!(pend_a, pend_b);
    assert_ne!(pend_a, g_pend);
    assert_ne!(sess_a, pend_a);
    assert_ne!(rec_idem_a, rec_a);
    assert_eq!(pend_a, layout_colon.project_pending_key(&ns_a));
    assert_eq!(g_pend, layout_colon.global_pending_key());
    assert_eq!(sess_a, layout_colon.session_index_key(&ns_a, &sess));
    assert_eq!(rec_idem_a, layout_colon.record_idem_key(&ns_a, mem_id));
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
    let store = Arc::new(
        RedisRecordStore::connect(RedisConfig::new(url, prefix))
            .await
            .expect("connect to live Redis/Valkey"),
    );

    assert_eq!(store.name(), "redis");
    assert_eq!(
        store.health_check().await.unwrap(),
        ConnectorHealth::Healthy
    );
    assert!(!format!("{store:?}").contains("s3cr3t"));

    muninn_aether_adapters::contract::record_store(store.as_ref()).await;
    muninn_aether_adapters::contract::record_store_concurrent_idempotency(Arc::clone(&store)).await;

    // Empty and zero-limit queries return empty without error.
    let empty_ns = ProjectId::new(format!("empty-{}", Uuid::new_v4())).unwrap();
    let sess0 = SessionId::new("redis-sess-0").unwrap();
    assert!(store.get_many(&empty_ns, &[]).await.unwrap().is_empty());
    assert!(store.pending(0).await.unwrap().is_empty());
    assert!(store.pending_for(&empty_ns, 0).await.unwrap().is_empty());
    assert!(store.pending_for(&empty_ns, 10).await.unwrap().is_empty());
    assert!(store
        .session_items(&empty_ns, &sess0, 0)
        .await
        .unwrap()
        .is_empty());
    assert!(store
        .session_items(&empty_ns, &sess0, 10)
        .await
        .unwrap()
        .is_empty());

    let ns = ProjectId::new(format!("edge-{}", Uuid::new_v4())).unwrap();

    // Missing / tombstoned CAS returns NotFound; missing tombstone returns None.
    let missing_id = MemoryId::new();
    let missing_item = MemoryItem::from_input(MemoryInput::new(ns.clone(), "missing"), 1);
    assert!(matches!(
        store
            .cas_update(&ns, missing_id, Some(1), &missing_item)
            .await,
        Err(AetherError::NotFound(_))
    ));
    assert!(store.tombstone(&ns, missing_id).await.unwrap().is_none());

    // Idempotency key TTL expiration allows re-claim with a new MemoryItem.
    let mut ttl_in1 = MemoryInput::new(ns.clone(), "expiring idem 1");
    ttl_in1.idempotency_key = Some("idem-ttl".into());
    let ttl_item1 = MemoryItem::from_input(ttl_in1, 1);

    let mut ttl_in2 = MemoryInput::new(ns.clone(), "reclaimed idem 2");
    ttl_in2.idempotency_key = Some("idem-ttl".into());
    let ttl_item2 = MemoryItem::from_input(ttl_in2, 1);

    assert_eq!(
        store
            .create(&ns, &ttl_item1, Some(Duration::from_millis(25)))
            .await
            .unwrap(),
        CreateOutcome::Created
    );
    assert!(matches!(
        store.create(&ns, &ttl_item2, None).await.unwrap(),
        CreateOutcome::Duplicate(r) if r.memory_id == ttl_item1.id && r.state == ReceiptState::Buffered
    ));
    tokio::time::sleep(Duration::from_millis(45)).await;
    assert_eq!(
        store.create(&ns, &ttl_item2, None).await.unwrap(),
        CreateOutcome::Created
    );
    assert!(matches!(
        store.create(&ns, &ttl_item1, None).await.unwrap(),
        CreateOutcome::Duplicate(r) if r.memory_id == ttl_item2.id
    ));

    // Zero-TTL create detects live idempotency key, otherwise expires immediately.
    let mut zero_dup_in = MemoryInput::new(ns.clone(), "zero ttl dup");
    zero_dup_in.idempotency_key = Some("idem-ttl".into());
    let zero_dup = MemoryItem::from_input(zero_dup_in, 1);
    assert!(matches!(
        store
            .create(&ns, &zero_dup, Some(Duration::ZERO))
            .await
            .unwrap(),
        CreateOutcome::Duplicate(r) if r.memory_id == ttl_item2.id
    ));

    let mut zero_fresh_in = MemoryInput::new(ns.clone(), "zero ttl fresh");
    zero_fresh_in.idempotency_key = Some("idem-zero-fresh".into());
    let zero_fresh = MemoryItem::from_input(zero_fresh_in, 1);
    assert_eq!(
        store
            .create(&ns, &zero_fresh, Some(Duration::ZERO))
            .await
            .unwrap(),
        CreateOutcome::Created
    );
    assert!(store.get(&ns, zero_fresh.id).await.unwrap().is_none());
    assert!(!store
        .pending_for(&ns, 100)
        .await
        .unwrap()
        .iter()
        .any(|x| x.id == zero_fresh.id));

    // CAS update preserves remaining TTL (PTTL) and re-queues into pending_for.
    let pttl_item = MemoryItem::from_input(MemoryInput::new(ns.clone(), "pttl v1"), 1);
    store
        .create(&ns, &pttl_item, Some(Duration::from_millis(80)))
        .await
        .unwrap();
    store.mark_indexed(&ns, pttl_item.id, 1).await.unwrap();

    let mut pttl_v2 = pttl_item.clone();
    pttl_v2.content = "pttl v2".into();
    pttl_v2.version = 2;
    store
        .cas_update(&ns, pttl_item.id, Some(1), &pttl_v2)
        .await
        .unwrap();
    assert_eq!(
        store.get(&ns, pttl_item.id).await.unwrap().unwrap().content,
        "pttl v2"
    );
    assert!(store
        .pending_for(&ns, 100)
        .await
        .unwrap()
        .iter()
        .any(|x| x.id == pttl_item.id && x.version == 2));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(store.get(&ns, pttl_item.id).await.unwrap().is_none());
    assert!(matches!(
        store.cas_update(&ns, pttl_item.id, Some(2), &pttl_v2).await,
        Err(AetherError::NotFound(_))
    ));

    // Session items newest-first ordering, get_many filtering, and CAS conflict/None.
    let sess = SessionId::new("redis-sess").unwrap();
    let mut s_old_in = MemoryInput::new(ns.clone(), "older session item");
    s_old_in.scope = MemoryScope::Session(sess.clone());
    let s_old = MemoryItem::from_input(s_old_in, 1);

    let mut s_new_in = MemoryInput::new(ns.clone(), "newer session item");
    s_new_in.scope = MemoryScope::Session(sess.clone());
    let s_new = MemoryItem::from_input(s_new_in, 1);

    store.create(&ns, &s_old, None).await.unwrap();
    store.create(&ns, &s_new, None).await.unwrap();

    let sess_ordered = store.session_items(&ns, &sess, 10).await.unwrap();
    let sess_ids: Vec<MemoryId> = sess_ordered.into_iter().map(|x| x.id).collect();
    assert_eq!(sess_ids, vec![s_new.id, s_old.id]);

    // Unconditional cas_update (expected_version = None) and conflict inspection.
    let mut s_old_v2 = s_old.clone();
    s_old_v2.content = "updated without expected version".into();
    s_old_v2.version = 2;
    store
        .cas_update(&ns, s_old.id, None, &s_old_v2)
        .await
        .unwrap();
    let conflict = store.cas_update(&ns, s_old.id, Some(1), &s_old_v2).await;
    assert!(matches!(
        conflict,
        Err(AetherError::Conflict {
            memory_id,
            expected_version: 1,
            actual_version: 2,
        }) if memory_id == s_old.id
    ));

    // Concurrent CAS updates on same version allow exactly one winner.
    let mut cas_handles = Vec::with_capacity(8);
    for idx in 0..8 {
        let store = Arc::clone(&store);
        let ns = ns.clone();
        let mut next = s_new.clone();
        next.content = format!("winner-{idx}");
        next.version = 2;
        cas_handles.push(tokio::spawn(async move {
            store.cas_update(&ns, s_new.id, Some(1), &next).await
        }));
    }
    let mut ok_count = 0usize;
    let mut conflict_count = 0usize;
    for handle in cas_handles {
        match handle.await.unwrap() {
            Ok(()) => ok_count += 1,
            Err(AetherError::Conflict {
                expected_version: 1,
                actual_version: 2,
                ..
            }) => conflict_count += 1,
            Err(other) => panic!("unexpected concurrent CAS error: {other:?}"),
        }
    }
    assert_eq!(ok_count, 1);
    assert_eq!(conflict_count, 7);

    // Tombstoned item is excluded from get_many and cannot be CAS-updated.
    store.tombstone(&ns, s_new.id).await.unwrap().unwrap();
    assert!(matches!(
        store.cas_update(&ns, s_new.id, None, &s_new).await,
        Err(AetherError::NotFound(_))
    ));
    let batch = store
        .get_many(&ns, &[s_old.id, s_new.id, missing_id])
        .await
        .unwrap();
    assert_eq!(batch.len(), 1);
    assert_eq!(batch[0].id, s_old.id);

    store.close().await.unwrap();
}
