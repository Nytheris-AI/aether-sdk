use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use aether_adapters::contract;
use aether_adapters::kv::sqlite::{SqliteConfig, SqliteRecordStore};
use aether_adapters::middleware::{Resilient, RetryPolicy, Timeout, Traced};
use aether_core::{
    AetherError, ConnectorHealth, MemoryId, MemoryInput, MemoryItem, MemoryReceipt, MemoryScope,
    OutboundConnector, ProjectId, ReceiptState, RecordStore, SessionId,
};
use uuid::Uuid;

struct TempDb {
    path: PathBuf,
}

impl TempDb {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!("aether-{label}-{}.db", Uuid::new_v4()));
        Self { path }
    }

    fn path_str(&self) -> String {
        self.path.to_string_lossy().into_owned()
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        let wal = PathBuf::from(format!("{}-wal", self.path.display()));
        let shm = PathBuf::from(format!("{}-shm", self.path.display()));
        let _ = std::fs::remove_file(wal);
        let _ = std::fs::remove_file(shm);
    }
}

fn fresh_project(prefix: &str) -> ProjectId {
    ProjectId::new(format!("{prefix}-{}", Uuid::new_v4())).expect("valid project id")
}

#[tokio::test]
async fn contract_record_store_sqlite_in_memory() {
    let store = SqliteRecordStore::open(SqliteConfig::in_memory())
        .await
        .expect("open in-memory sqlite store");
    assert_eq!(store.name(), "sqlite");
    assert_eq!(
        store.health_check().await.expect("health check"),
        ConnectorHealth::Healthy
    );

    contract::record_store(&store).await;

    store.close().await.expect("close store");
}

#[tokio::test]
async fn contract_record_store_sqlite_file_backed() {
    let db = TempDb::new("contract");
    let store = SqliteRecordStore::open(SqliteConfig::from_path(db.path_str()))
        .await
        .expect("open file-backed sqlite store");

    contract::record_store(&store).await;

    store.close().await.expect("close store");
}

#[tokio::test]
async fn contract_record_store_sqlite_through_middleware() {
    let db = TempDb::new("middleware");
    let store = Arc::new(
        SqliteRecordStore::open(SqliteConfig::from_path(db.path_str()))
            .await
            .expect("open sqlite store"),
    );
    let wrapped = Traced::new(Resilient::new(
        Timeout::new(store, Duration::from_secs(2)),
        RetryPolicy::default(),
    ));

    assert_eq!(wrapped.name(), "sqlite");
    assert_eq!(
        wrapped.health_check().await.expect("health check"),
        ConnectorHealth::Healthy
    );

    contract::record_store(&wrapped).await;

    wrapped.close().await.expect("close wrapped store");
}

#[tokio::test]
async fn sqlite_persists_records_and_idempotency_across_reopen() {
    let db = TempDb::new("persist");
    let ns = fresh_project("persist");

    let mut first_input = MemoryInput::new(ns.clone(), "first persisted memory");
    first_input.scope = MemoryScope::Project;
    let first = MemoryItem::from_input(first_input, 1);

    let sess = SessionId::new("sqlite-sess").unwrap();
    let mut second_input = MemoryInput::new(ns.clone(), "second persisted memory");
    second_input.scope = MemoryScope::Session(sess.clone());
    let second = MemoryItem::from_input(second_input, 1);

    let mut expiring_input = MemoryInput::new(ns.clone(), "short lived memory");
    expiring_input.scope = MemoryScope::Session(sess);
    let expiring = MemoryItem::from_input(expiring_input, 1);

    let receipt = MemoryReceipt {
        memory_id: first.id,
        state: ReceiptState::Persisted,
        version: 1,
    };

    {
        let store = SqliteRecordStore::open(SqliteConfig::from_path(db.path_str()))
            .await
            .expect("open initial store");
        store.put(&ns, &first, None).await.expect("put first");
        store.put(&ns, &second, None).await.expect("put second");
        store
            .put(&ns, &expiring, Some(Duration::from_millis(20)))
            .await
            .expect("put expiring");
        assert!(store
            .put_if_absent(&ns, "idem-persist", &receipt, None)
            .await
            .expect("claim idempotency key")
            .is_none());
        store.close().await.expect("close initial store");
    }

    tokio::time::sleep(Duration::from_millis(35)).await;

    let reopened = SqliteRecordStore::open(SqliteConfig::from_path(db.path_str()))
        .await
        .expect("reopen store");

    assert_eq!(
        reopened.get(&ns, first.id).await.expect("get first"),
        Some(first.clone())
    );
    assert_eq!(
        reopened.get(&ns, second.id).await.expect("get second"),
        Some(second.clone())
    );
    assert_eq!(
        reopened.get(&ns, expiring.id).await.expect("get expiring"),
        None
    );

    let dup = reopened
        .put_if_absent(
            &ns,
            "idem-persist",
            &MemoryReceipt {
                memory_id: MemoryId::new(),
                state: ReceiptState::Buffered,
                version: 9,
            },
            None,
        )
        .await
        .expect("re-check idempotency key");
    assert_eq!(dup, Some(receipt));

    let mut third_input = MemoryInput::new(ns.clone(), "third post-reopen memory");
    third_input.scope = MemoryScope::Project;
    let third = MemoryItem::from_input(third_input, 1);
    reopened.put(&ns, &third, None).await.expect("put third");

    let recent_all = reopened.recent(&ns, None, 10).await.expect("recent all");
    let recent_ids: Vec<MemoryId> = recent_all.into_iter().map(|item| item.id).collect();
    assert_eq!(recent_ids, vec![third.id, second.id, first.id]);

    let mut updated_first = first.clone();
    updated_first.content = "updated after reopen".into();
    updated_first.version = 2;
    reopened
        .cas_update(&ns, first.id, Some(1), &updated_first)
        .await
        .expect("cas_update persisted item");
    assert_eq!(
        reopened
            .get(&ns, first.id)
            .await
            .expect("get updated first"),
        Some(updated_first)
    );

    reopened.close().await.expect("close reopened store");
}

#[tokio::test]
async fn sqlite_error_paths_and_edge_cases() {
    let invalid = SqliteRecordStore::open(SqliteConfig::from_path("   ")).await;
    assert!(matches!(invalid, Err(AetherError::InvalidInput(_))));

    let store = SqliteRecordStore::open(SqliteConfig::in_memory())
        .await
        .expect("open in-memory store");
    let ns = fresh_project("errors");

    let missing_id = MemoryId::new();
    let missing_item = MemoryItem::from_input(MemoryInput::new(ns.clone(), "missing"), 1);
    let not_found = store
        .cas_update(&ns, missing_id, Some(1), &missing_item)
        .await;
    assert!(matches!(not_found, Err(AetherError::NotFound(_))));

    // Idempotent delete on missing record succeeds.
    store.delete(&ns, missing_id).await.expect("delete missing");

    // Expired record cannot be CAS-updated.
    let ttl_item = MemoryItem::from_input(MemoryInput::new(ns.clone(), "ephemeral"), 1);
    store
        .put(&ns, &ttl_item, Some(Duration::from_millis(15)))
        .await
        .expect("put ephemeral");
    tokio::time::sleep(Duration::from_millis(30)).await;
    let mut ttl_v2 = ttl_item.clone();
    ttl_v2.version = 2;
    let expired_cas = store.cas_update(&ns, ttl_item.id, Some(1), &ttl_v2).await;
    assert!(matches!(expired_cas, Err(AetherError::NotFound(_))));

    // Conflict reports expected and actual versions; expected_version=None skips version check.
    let item = MemoryItem::from_input(MemoryInput::new(ns.clone(), "v1"), 1);
    store.put(&ns, &item, None).await.expect("put v1");
    let mut v2 = item.clone();
    v2.version = 2;
    v2.content = "v2".into();
    let conflict = store.cas_update(&ns, item.id, Some(99), &v2).await;
    assert!(matches!(
        conflict,
        Err(AetherError::Conflict {
            memory_id,
            expected_version: 99,
            actual_version: 1,
        }) if memory_id == item.id
    ));
    store
        .cas_update(&ns, item.id, None, &v2)
        .await
        .expect("unconditional cas_update");
    assert_eq!(
        store
            .get(&ns, item.id)
            .await
            .expect("get v2")
            .expect("item present")
            .version,
        2
    );

    // Expired idempotency key can be re-claimed.
    let r1 = MemoryReceipt {
        memory_id: MemoryId::new(),
        state: ReceiptState::Buffered,
        version: 1,
    };
    let r2 = MemoryReceipt {
        memory_id: MemoryId::new(),
        state: ReceiptState::Persisted,
        version: 1,
    };
    assert!(store
        .put_if_absent(&ns, "idem-ttl", &r1, Some(Duration::from_millis(15)))
        .await
        .expect("claim r1")
        .is_none());
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(store
        .put_if_absent(&ns, "idem-ttl", &r2, None)
        .await
        .expect("re-claim after expiry")
        .is_none());
    assert_eq!(
        store
            .put_if_absent(&ns, "idem-ttl", &r1, None)
            .await
            .expect("read r2")
            .as_ref(),
        Some(&r2)
    );

    // Zero limit returns empty vec without error.
    assert!(store
        .recent(&ns, None, 0)
        .await
        .expect("recent limit 0")
        .is_empty());

    // Closing makes subsequent operations fail cleanly and health_check report Unhealthy.
    store.close().await.expect("first close");
    store.close().await.expect("idempotent second close");
    assert!(matches!(
        store.health_check().await.expect("health after close"),
        ConnectorHealth::Unhealthy(_)
    ));
    assert!(matches!(
        store.get(&ns, item.id).await,
        Err(AetherError::Permanent {
            connector: "sqlite",
            ..
        })
    ));
}

#[tokio::test]
async fn sqlite_concurrent_put_if_absent_and_cas_are_atomic() {
    let db = TempDb::new("concurrent");
    let store = Arc::new(
        SqliteRecordStore::open(SqliteConfig::from_path(db.path_str()))
            .await
            .expect("open sqlite store"),
    );
    let ns = fresh_project("concurrent");

    let mut idem_handles = Vec::new();
    for _ in 0..8 {
        let store = Arc::clone(&store);
        let ns = ns.clone();
        let candidate = MemoryReceipt {
            memory_id: MemoryId::new(),
            state: ReceiptState::Buffered,
            version: 1,
        };
        idem_handles.push(tokio::spawn(async move {
            (
                candidate.memory_id,
                store
                    .put_if_absent(&ns, "race-key", &candidate, None)
                    .await
                    .expect("put_if_absent"),
            )
        }));
    }

    let mut claimed = 0usize;
    let mut observed_ids = Vec::new();
    for handle in idem_handles {
        let (candidate_id, existing) = handle.await.expect("join idem task");
        match existing {
            None => {
                claimed += 1;
                observed_ids.push(candidate_id);
            }
            Some(receipt) => observed_ids.push(receipt.memory_id),
        }
    }
    assert_eq!(claimed, 1);
    assert!(observed_ids.windows(2).all(|w| w[0] == w[1]));

    let base = MemoryItem::from_input(MemoryInput::new(ns.clone(), "initial"), 1);
    store.put(&ns, &base, None).await.expect("put base");

    let mut cas_handles = Vec::new();
    for idx in 0..8u64 {
        let store = Arc::clone(&store);
        let ns = ns.clone();
        let mut next = base.clone();
        next.content = format!("winner-{idx}");
        next.version = 2;
        cas_handles.push(tokio::spawn(async move {
            store.cas_update(&ns, base.id, Some(1), &next).await
        }));
    }

    let mut ok_count = 0usize;
    let mut conflict_count = 0usize;
    for handle in cas_handles {
        match handle.await.expect("join cas task") {
            Ok(()) => ok_count += 1,
            Err(AetherError::Conflict {
                expected_version: 1,
                actual_version: 2,
                ..
            }) => conflict_count += 1,
            Err(other) => panic!("unexpected cas error: {other:?}"),
        }
    }
    assert_eq!(ok_count, 1);
    assert_eq!(conflict_count, 7);

    store.close().await.expect("close store");
}
