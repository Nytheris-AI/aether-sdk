use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use nytheris_aether_adapters::contract;
use nytheris_aether_adapters::kv::sqlite::{SqliteConfig, SqliteRecordStore};
use nytheris_aether_adapters::middleware::{Resilient, RetryPolicy, Timeout, Traced};
use nytheris_aether_core::{
    AetherError, ConnectorHealth, CreateOutcome, MemoryId, MemoryInput, MemoryItem, MemoryReceipt,
    MemoryScope, OutboundConnector, ProjectId, ReceiptState, RecordStore, SessionId,
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
    first_input.idempotency_key = Some("idem-persist".into());
    let first = MemoryItem::from_input(first_input, 1);

    let sess = SessionId::new("sqlite-sess").unwrap();
    let mut second_input = MemoryInput::new(ns.clone(), "second persisted memory");
    second_input.scope = MemoryScope::Session(sess.clone());
    let second = MemoryItem::from_input(second_input, 1);

    let mut expiring_input = MemoryInput::new(ns.clone(), "short lived memory");
    expiring_input.scope = MemoryScope::Session(sess.clone());
    let expiring = MemoryItem::from_input(expiring_input, 1);

    {
        let store = SqliteRecordStore::open(SqliteConfig::from_path(db.path_str()))
            .await
            .expect("open initial store");
        assert_eq!(
            store.create(&ns, &first, None).await.expect("create first"),
            CreateOutcome::Created
        );
        store
            .mark_indexed(&ns, first.id, 1)
            .await
            .expect("mark first indexed");
        assert_eq!(
            store
                .create(&ns, &second, None)
                .await
                .expect("create second"),
            CreateOutcome::Created
        );
        assert_eq!(
            store
                .create(&ns, &expiring, Some(Duration::from_millis(20)))
                .await
                .expect("create expiring"),
            CreateOutcome::Created
        );
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

    let mut dup_input = MemoryInput::new(ns.clone(), "duplicate payload");
    dup_input.idempotency_key = Some("idem-persist".into());
    let dup_item = MemoryItem::from_input(dup_input, 1);
    let dup = reopened
        .create(&ns, &dup_item, None)
        .await
        .expect("re-check idempotency key");
    assert_eq!(
        dup,
        CreateOutcome::Duplicate(MemoryReceipt {
            memory_id: first.id,
            state: ReceiptState::Persisted,
            version: 1,
        })
    );

    let mut third_input = MemoryInput::new(ns.clone(), "third post-reopen memory");
    third_input.scope = MemoryScope::Session(sess.clone());
    let third = MemoryItem::from_input(third_input, 1);
    reopened
        .create(&ns, &third, None)
        .await
        .expect("create third");

    let sess_all = reopened
        .session_items(&ns, &sess, 10)
        .await
        .expect("session items");
    let sess_ids: Vec<MemoryId> = sess_all.into_iter().map(|item| item.id).collect();
    assert_eq!(sess_ids, vec![third.id, second.id]);

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
    assert!(reopened
        .pending_for(&ns, 10)
        .await
        .expect("pending_for updated")
        .iter()
        .any(|item| item.id == first.id && item.version == 2));

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

    // Idempotent tombstone on missing record returns None.
    assert!(store
        .tombstone(&ns, missing_id)
        .await
        .expect("tombstone missing")
        .is_none());

    // Expired record cannot be CAS-updated.
    let ttl_item = MemoryItem::from_input(MemoryInput::new(ns.clone(), "ephemeral"), 1);
    store
        .create(&ns, &ttl_item, Some(Duration::from_millis(15)))
        .await
        .expect("create ephemeral");
    tokio::time::sleep(Duration::from_millis(30)).await;
    let mut ttl_v2 = ttl_item.clone();
    ttl_v2.version = 2;
    let expired_cas = store.cas_update(&ns, ttl_item.id, Some(1), &ttl_v2).await;
    assert!(matches!(expired_cas, Err(AetherError::NotFound(_))));

    // Conflict reports expected and actual versions; expected_version=None skips version check.
    let item = MemoryItem::from_input(MemoryInput::new(ns.clone(), "v1"), 1);
    store.create(&ns, &item, None).await.expect("create v1");
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
    let mut r1_in = MemoryInput::new(ns.clone(), "expiring key 1");
    r1_in.idempotency_key = Some("idem-ttl".into());
    let r1 = MemoryItem::from_input(r1_in, 1);

    let mut r2_in = MemoryInput::new(ns.clone(), "reclaimed key 2");
    r2_in.idempotency_key = Some("idem-ttl".into());
    let r2 = MemoryItem::from_input(r2_in, 1);

    assert_eq!(
        store
            .create(&ns, &r1, Some(Duration::from_millis(15)))
            .await
            .expect("claim r1"),
        CreateOutcome::Created
    );
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(
        store
            .create(&ns, &r2, None)
            .await
            .expect("re-claim after expiry"),
        CreateOutcome::Created
    );
    assert_eq!(
        store.create(&ns, &r1, None).await.expect("read r2"),
        CreateOutcome::Duplicate(MemoryReceipt {
            memory_id: r2.id,
            state: ReceiptState::Buffered,
            version: 1,
        })
    );

    // Zero limit returns empty vec without error.
    assert!(store.pending(0).await.expect("pending limit 0").is_empty());
    assert!(store
        .pending_for(&ns, 0)
        .await
        .expect("pending_for limit 0")
        .is_empty());
    let s0 = SessionId::new("s0").unwrap();
    assert!(store
        .session_items(&ns, &s0, 0)
        .await
        .expect("session_items limit 0")
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
async fn sqlite_concurrent_create_and_cas_are_atomic() {
    let db = TempDb::new("concurrent");
    let store = Arc::new(
        SqliteRecordStore::open(SqliteConfig::from_path(db.path_str()))
            .await
            .expect("open sqlite store"),
    );
    contract::record_store_concurrent_idempotency(Arc::clone(&store)).await;

    let ns = fresh_project("concurrent-cas");
    let base = MemoryItem::from_input(MemoryInput::new(ns.clone(), "initial"), 1);
    store.create(&ns, &base, None).await.expect("create base");

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

#[tokio::test]
async fn sqlite_migrates_pre_phase5_schema_cleanly() {
    let db = TempDb::new("legacy-migrate");
    let ns = fresh_project("legacy");
    let legacy_mem_id = MemoryId::new();
    {
        let conn = rusqlite::Connection::open(db.path_str()).expect("open raw legacy sqlite");
        conn.execute_batch(
            "CREATE TABLE records (
                 project_id TEXT NOT NULL,
                 memory_id TEXT NOT NULL,
                 scope TEXT NOT NULL,
                 version INTEGER NOT NULL,
                 seq INTEGER NOT NULL,
                 expires_at_us INTEGER,
                 payload TEXT NOT NULL,
                 PRIMARY KEY (project_id, memory_id)
             );
             CREATE TABLE idempotency_keys (
                 project_id TEXT NOT NULL,
                 idempotency_key TEXT NOT NULL,
                 expires_at_us INTEGER,
                 receipt_json TEXT NOT NULL,
                 PRIMARY KEY (project_id, idempotency_key)
             );",
        )
        .expect("create legacy schema");
        let legacy_item = MemoryItem::from_input(MemoryInput::new(ns.clone(), "legacy row"), 1);
        let payload = serde_json::to_string(&legacy_item).unwrap();
        let receipt_json = serde_json::json!({
            "memory_id": legacy_mem_id,
            "state": "persisted",
            "version": 1
        })
        .to_string();
        conn.execute(
            "INSERT INTO records (project_id, memory_id, scope, version, seq, expires_at_us, payload)
             VALUES (?1, ?2, 'project', 1, 1, NULL, ?3)",
            rusqlite::params![ns.as_str(), legacy_item.id.to_string(), payload],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO idempotency_keys (project_id, idempotency_key, expires_at_us, receipt_json)
             VALUES (?1, 'old-key', NULL, ?2)",
            rusqlite::params![ns.as_str(), receipt_json],
        )
        .unwrap();
    }

    let store = SqliteRecordStore::open(SqliteConfig::from_path(db.path_str()))
        .await
        .expect("open migrated sqlite store");

    let mut input = MemoryInput::new(ns.clone(), "post migration insert");
    input.idempotency_key = Some("new-key".into());
    let item = MemoryItem::from_input(input, 1);
    assert_eq!(
        store
            .create(&ns, &item, None)
            .await
            .expect("create after migration"),
        CreateOutcome::Created
    );
    assert!(store.get(&ns, item.id).await.unwrap().is_some());
    store.close().await.unwrap();
}
