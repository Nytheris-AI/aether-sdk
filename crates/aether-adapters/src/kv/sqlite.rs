//! SQLite WAL-mode `RecordStore` connector.

use std::collections::HashSet;
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use chrono::Utc;
use muninn_aether_core::{
    AetherError, ConnectorCapabilities, ConnectorHealth, CreateOutcome, MemoryId, MemoryItem,
    MemoryReceipt, MemoryScope, OutboundConnector, PendingRecord, ProjectId, ReceiptState,
    RecordStore, Result, SessionId,
};
use rusqlite::{
    params, params_from_iter, Connection, ErrorCode, OptionalExtension, Transaction,
    TransactionBehavior,
};
use uuid::Uuid;

const CONNECTOR_NAME: &str = "sqlite";

const INIT_SQL: &str = "
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA busy_timeout = 5000;

CREATE TABLE IF NOT EXISTS records (
    project_id TEXT NOT NULL,
    memory_id TEXT NOT NULL,
    scope TEXT NOT NULL,
    version INTEGER NOT NULL,
    indexed_version INTEGER NOT NULL DEFAULT 0,
    tombstone INTEGER NOT NULL DEFAULT 0,
    seq INTEGER NOT NULL,
    expires_at_us INTEGER,
    payload TEXT NOT NULL,
    PRIMARY KEY (project_id, memory_id)
);
CREATE INDEX IF NOT EXISTS idx_records_recent
    ON records (project_id, scope, seq DESC);
CREATE INDEX IF NOT EXISTS idx_records_project_seq
    ON records (project_id, seq DESC);
CREATE INDEX IF NOT EXISTS idx_records_seq
    ON records (seq ASC);

CREATE TABLE IF NOT EXISTS idempotency_keys (
    project_id TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    memory_id TEXT NOT NULL,
    expires_at_us INTEGER,
    PRIMARY KEY (project_id, idempotency_key)
);

CREATE TABLE IF NOT EXISTS seq_counter (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    val INTEGER NOT NULL
);
INSERT OR IGNORE INTO seq_counter (id, val) VALUES (1, 0);
";

/// Configuration for [`SqliteRecordStore`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqliteConfig {
    pub path: String,
}

impl SqliteConfig {
    /// Creates an in-memory SQLite configuration (`:memory:`).
    pub fn in_memory() -> Self {
        Self {
            path: ":memory:".to_string(),
        }
    }

    /// Creates a file-backed SQLite configuration at `path`.
    pub fn from_path(path: impl Into<String>) -> Self {
        Self { path: path.into() }
    }
}

/// Persistent or in-memory SQLite WAL-mode implementation of [`RecordStore`].
#[derive(Clone)]
pub struct SqliteRecordStore {
    conn: Arc<Mutex<Option<Connection>>>,
}

impl fmt::Debug for SqliteRecordStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let is_open = self
            .conn
            .lock()
            .map(|guard| guard.is_some())
            .unwrap_or(false);
        f.debug_struct("SqliteRecordStore")
            .field("open", &is_open)
            .finish()
    }
}

impl SqliteRecordStore {
    /// Opens a SQLite database, enables WAL mode, and initializes the record schema.
    pub async fn open(config: SqliteConfig) -> Result<Self> {
        let path = config.path.trim().to_string();
        if path.is_empty() {
            return Err(AetherError::InvalidInput(
                "sqlite path must not be empty".into(),
            ));
        }

        let conn = tokio::task::spawn_blocking(move || -> Result<Connection> {
            let conn = if path == ":memory:" {
                Connection::open_in_memory().map_err(map_sqlite_err)?
            } else {
                Connection::open(&path).map_err(map_sqlite_err)?
            };
            conn.execute_batch(INIT_SQL).map_err(map_sqlite_err)?;
            ensure_schema(&conn).map_err(map_sqlite_err)?;
            Ok(conn)
        })
        .await
        .map_err(|err| permanent_err(format!("sqlite task join error: {err}")))??;

        Ok(Self {
            conn: Arc::new(Mutex::new(Some(conn))),
        })
    }

    async fn with_conn<F, R>(&self, op: F) -> Result<R>
    where
        F: FnOnce(&mut Connection) -> Result<R> + Send + 'static,
        R: Send + 'static,
    {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let mut guard = conn
                .lock()
                .map_err(|_| permanent_err("sqlite connection mutex poisoned"))?;
            let conn = guard
                .as_mut()
                .ok_or_else(|| permanent_err("sqlite connection is closed"))?;
            op(conn)
        })
        .await
        .map_err(|err| permanent_err(format!("sqlite task join error: {err}")))?
    }
}

fn ensure_schema(conn: &Connection) -> rusqlite::Result<()> {
    let mut rec_cols = HashSet::new();
    let mut stmt = conn.prepare("PRAGMA table_info(records)")?;
    for row in stmt.query_map([], |r| r.get::<_, String>(1))? {
        rec_cols.insert(row?);
    }
    if !rec_cols.contains("indexed_version") {
        conn.execute(
            "ALTER TABLE records ADD COLUMN indexed_version INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }
    if !rec_cols.contains("tombstone") {
        conn.execute(
            "ALTER TABLE records ADD COLUMN tombstone INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }

    let mut idem_cols = HashSet::new();
    let mut idem_stmt = conn.prepare("PRAGMA table_info(idempotency_keys)")?;
    for row in idem_stmt.query_map([], |r| r.get::<_, String>(1))? {
        idem_cols.insert(row?);
    }
    if !idem_cols.contains("memory_id") {
        conn.execute(
            "ALTER TABLE idempotency_keys ADD COLUMN memory_id TEXT NOT NULL DEFAULT ''",
            [],
        )?;
    }
    if idem_cols.contains("receipt_json") {
        conn.execute(
            "UPDATE idempotency_keys
             SET memory_id = COALESCE(json_extract(receipt_json, '$.memory_id'), '')
             WHERE memory_id = ''",
            [],
        )?;
        conn.execute("ALTER TABLE idempotency_keys DROP COLUMN receipt_json", [])?;
    }
    conn.execute(
        "UPDATE seq_counter SET val = MAX(val, COALESCE((SELECT MAX(seq) FROM records), 0)) WHERE id = 1",
        [],
    )?;
    Ok(())
}

#[async_trait]
impl OutboundConnector for SqliteRecordStore {
    fn name(&self) -> &'static str {
        CONNECTOR_NAME
    }

    fn capabilities(&self) -> ConnectorCapabilities {
        ConnectorCapabilities {
            max_batch_size: 512,
            supports_temporal: false,
            supports_filter_expr: false,
        }
    }

    async fn health_check(&self) -> Result<ConnectorHealth> {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let guard = conn
                .lock()
                .map_err(|_| permanent_err("sqlite connection mutex poisoned"))?;
            let Some(conn) = guard.as_ref() else {
                return Ok(ConnectorHealth::Unhealthy(
                    "sqlite connection is closed".into(),
                ));
            };
            match conn.query_row("SELECT 1", [], |row| row.get::<_, i64>(0)) {
                Ok(1) => Ok(ConnectorHealth::Healthy),
                Ok(other) => Ok(ConnectorHealth::Unhealthy(format!(
                    "unexpected SELECT 1 result: {other}"
                ))),
                Err(err) => Ok(ConnectorHealth::Unhealthy(err.to_string())),
            }
        })
        .await
        .map_err(|err| permanent_err(format!("sqlite task join error: {err}")))?
    }

    async fn close(&self) -> Result<()> {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let mut guard = conn
                .lock()
                .map_err(|_| permanent_err("sqlite connection mutex poisoned"))?;
            let _ = guard.take();
            Ok(())
        })
        .await
        .map_err(|err| permanent_err(format!("sqlite task join error: {err}")))?
    }
}

#[async_trait]
impl RecordStore for SqliteRecordStore {
    async fn create(
        &self,
        ns: &ProjectId,
        item: &MemoryItem,
        ttl: Option<Duration>,
    ) -> Result<CreateOutcome> {
        let ns_str = ns.as_str().to_string();
        let id_str = item.id.to_string();
        let scope = scope_as_str(&item.scope);
        let version = item.version as i64;
        let idem_key = item.idempotency_key.clone();
        let payload = serde_json::to_string(item)?;
        let now_us = now_epoch_us();
        let exp_us = expires_at_us(now_us, ttl);

        self.with_conn(move |conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(map_sqlite_err)?;

            tx.execute(
                "DELETE FROM records WHERE expires_at_us IS NOT NULL AND expires_at_us <= ?1",
                params![now_us],
            )
            .map_err(map_sqlite_err)?;
            tx.execute(
                "DELETE FROM idempotency_keys WHERE expires_at_us IS NOT NULL AND expires_at_us <= ?1",
                params![now_us],
            )
            .map_err(map_sqlite_err)?;

            if let Some(key) = &idem_key {
                let existing_mem: Option<String> = tx
                    .query_row(
                        "SELECT memory_id FROM idempotency_keys
                         WHERE project_id = ?1 AND idempotency_key = ?2",
                        params![ns_str, key],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(map_sqlite_err)?;

                if let Some(existing_id_str) = existing_mem {
                    let rec_state: Option<(i64, i64, i64)> = tx
                        .query_row(
                            "SELECT version, indexed_version, tombstone FROM records
                             WHERE project_id = ?1 AND memory_id = ?2",
                            params![ns_str, existing_id_str],
                            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                        )
                        .optional()
                        .map_err(map_sqlite_err)?;

                    if let Some((ver, idx_ver, tomb)) = rec_state {
                        if tomb == 0 {
                            let parsed = Uuid::parse_str(&existing_id_str)
                                .map(MemoryId::from_uuid)
                                .map_err(|err| permanent_err(format!("corrupt memory_id: {err}")))?;
                            let state = if idx_ver >= ver {
                                ReceiptState::Persisted
                            } else {
                                ReceiptState::Buffered
                            };
                            return Ok(CreateOutcome::Duplicate(MemoryReceipt {
                                memory_id: parsed,
                                state,
                                version: ver as u64,
                            }));
                        }
                    }

                    tx.execute(
                        "DELETE FROM idempotency_keys WHERE project_id = ?1 AND idempotency_key = ?2",
                        params![ns_str, key],
                    )
                    .map_err(map_sqlite_err)?;
                }
            }

            let seq = next_seq(&tx).map_err(map_sqlite_err)?;
            tx.execute(
                "INSERT INTO records (
                     project_id, memory_id, scope, version, indexed_version, tombstone, seq, expires_at_us, payload
                 )
                 VALUES (?1, ?2, ?3, ?4, 0, 0, ?5, ?6, ?7)
                 ON CONFLICT(project_id, memory_id) DO UPDATE SET
                     scope = excluded.scope,
                     version = excluded.version,
                     indexed_version = 0,
                     tombstone = 0,
                     seq = excluded.seq,
                     expires_at_us = excluded.expires_at_us,
                     payload = excluded.payload",
                params![ns_str, id_str, scope, version, seq, exp_us, payload],
            )
            .map_err(map_sqlite_err)?;

            if let Some(key) = idem_key {
                tx.execute(
                    "INSERT INTO idempotency_keys (project_id, idempotency_key, memory_id, expires_at_us)
                     VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(project_id, idempotency_key) DO UPDATE SET
                         memory_id = excluded.memory_id,
                         expires_at_us = excluded.expires_at_us",
                    params![ns_str, key, id_str, exp_us],
                )
                .map_err(map_sqlite_err)?;
            }

            tx.commit().map_err(map_sqlite_err)?;
            Ok(CreateOutcome::Created)
        })
        .await
    }

    async fn get(&self, ns: &ProjectId, id: MemoryId) -> Result<Option<MemoryItem>> {
        let ns_str = ns.as_str().to_string();
        let id_str = id.to_string();
        let now_us = now_epoch_us();

        self.with_conn(move |conn| {
            let payload: Option<String> = conn
                .query_row(
                    "SELECT payload FROM records
                     WHERE project_id = ?1
                       AND memory_id = ?2
                       AND tombstone = 0
                       AND (expires_at_us IS NULL OR expires_at_us > ?3)",
                    params![ns_str, id_str, now_us],
                    |row| row.get(0),
                )
                .optional()
                .map_err(map_sqlite_err)?;

            payload
                .map(|json| serde_json::from_str::<MemoryItem>(&json).map_err(Into::into))
                .transpose()
        })
        .await
    }

    async fn get_many(&self, ns: &ProjectId, ids: &[MemoryId]) -> Result<Vec<MemoryItem>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let ns_str = ns.as_str().to_string();
        let id_strs: Vec<String> = ids.iter().map(ToString::to_string).collect();
        let now_us = now_epoch_us();

        self.with_conn(move |conn| {
            let mut items = Vec::new();
            for chunk in id_strs.chunks(256) {
                let placeholders: Vec<&str> = std::iter::repeat_n("?", chunk.len()).collect();
                let sql = format!(
                    "SELECT payload FROM records
                     WHERE project_id = ?
                       AND tombstone = 0
                       AND (expires_at_us IS NULL OR expires_at_us > ?)
                       AND memory_id IN ({})",
                    placeholders.join(", ")
                );
                let mut bound: Vec<rusqlite::types::Value> = Vec::with_capacity(chunk.len() + 2);
                bound.push(ns_str.clone().into());
                bound.push(now_us.into());
                for id in chunk {
                    bound.push(id.clone().into());
                }
                let mut stmt = conn.prepare(&sql).map_err(map_sqlite_err)?;
                let rows = stmt
                    .query_map(params_from_iter(bound), |row| row.get::<_, String>(0))
                    .map_err(map_sqlite_err)?;
                for row in rows {
                    let json = row.map_err(map_sqlite_err)?;
                    items.push(serde_json::from_str::<MemoryItem>(&json)?);
                }
            }
            Ok(items)
        })
        .await
    }

    async fn cas_update(
        &self,
        ns: &ProjectId,
        id: MemoryId,
        expected_version: Option<u64>,
        new_item: &MemoryItem,
    ) -> Result<()> {
        let ns_str = ns.as_str().to_string();
        let id_str = id.to_string();
        let scope = scope_as_str(&new_item.scope);
        let new_version = new_item.version as i64;
        let payload = serde_json::to_string(new_item)?;
        let now_us = now_epoch_us();

        self.with_conn(move |conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(map_sqlite_err)?;

            let current: Option<(i64, i64, Option<i64>)> = tx
                .query_row(
                    "SELECT version, tombstone, expires_at_us
                     FROM records
                     WHERE project_id = ?1 AND memory_id = ?2",
                    params![ns_str, id_str],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()
                .map_err(map_sqlite_err)?;

            let Some((actual_version_i64, tombstone, existing_exp)) = current else {
                return Err(AetherError::NotFound(format!("memory {id}")));
            };
            if tombstone != 0 || existing_exp.is_some_and(|exp| exp <= now_us) {
                return Err(AetherError::NotFound(format!("memory {id}")));
            }

            let actual_version = actual_version_i64 as u64;
            if let Some(expected) = expected_version {
                if actual_version != expected {
                    return Err(AetherError::Conflict {
                        memory_id: id,
                        expected_version: expected,
                        actual_version,
                    });
                }
            }

            let seq = next_seq(&tx).map_err(map_sqlite_err)?;
            tx.execute(
                "UPDATE records
                 SET scope = ?1, version = ?2, seq = ?3, payload = ?4
                 WHERE project_id = ?5 AND memory_id = ?6",
                params![scope, new_version, seq, payload, ns_str, id_str],
            )
            .map_err(map_sqlite_err)?;
            tx.commit().map_err(map_sqlite_err)?;
            Ok(())
        })
        .await
    }

    async fn tombstone(&self, ns: &ProjectId, id: MemoryId) -> Result<Option<MemoryItem>> {
        let ns_str = ns.as_str().to_string();
        let id_str = id.to_string();
        let now_us = now_epoch_us();

        self.with_conn(move |conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(map_sqlite_err)?;

            let current: Option<(String, i64, i64, Option<i64>)> = tx
                .query_row(
                    "SELECT payload, version, tombstone, expires_at_us
                     FROM records
                     WHERE project_id = ?1 AND memory_id = ?2",
                    params![ns_str, id_str],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .optional()
                .map_err(map_sqlite_err)?;

            let Some((payload, version_i64, tombstone, existing_exp)) = current else {
                return Ok(None);
            };
            if tombstone != 0 || existing_exp.is_some_and(|exp| exp <= now_us) {
                return Ok(None);
            }

            let mut item: MemoryItem = serde_json::from_str(&payload)?;
            item.version = (version_i64 as u64).saturating_add(1);
            item.updated_at = Utc::now();
            let updated_payload = serde_json::to_string(&item)?;
            let seq = next_seq(&tx).map_err(map_sqlite_err)?;

            tx.execute(
                "UPDATE records
                 SET version = ?1, tombstone = 1, seq = ?2, payload = ?3
                 WHERE project_id = ?4 AND memory_id = ?5",
                params![item.version as i64, seq, updated_payload, ns_str, id_str],
            )
            .map_err(map_sqlite_err)?;

            tx.execute(
                "DELETE FROM idempotency_keys WHERE project_id = ?1 AND memory_id = ?2",
                params![ns_str, id_str],
            )
            .map_err(map_sqlite_err)?;

            tx.commit().map_err(map_sqlite_err)?;
            Ok(Some(item))
        })
        .await
    }

    async fn pending(&self, limit: usize) -> Result<Vec<PendingRecord>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let now_us = now_epoch_us();
        let limit_i64 = i64::try_from(limit).unwrap_or(i64::MAX);

        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT payload, tombstone FROM records
                     WHERE version > indexed_version
                       AND (expires_at_us IS NULL OR expires_at_us > ?1)
                     ORDER BY seq ASC
                     LIMIT ?2",
                )
                .map_err(map_sqlite_err)?;
            let rows = stmt
                .query_map(params![now_us, limit_i64], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                })
                .map_err(map_sqlite_err)?;

            let mut out = Vec::new();
            for row in rows {
                let (json, tomb) = row.map_err(map_sqlite_err)?;
                out.push(PendingRecord {
                    item: serde_json::from_str::<MemoryItem>(&json)?,
                    tombstone: tomb != 0,
                });
            }
            Ok(out)
        })
        .await
    }

    async fn pending_for(&self, ns: &ProjectId, limit: usize) -> Result<Vec<MemoryItem>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let ns_str = ns.as_str().to_string();
        let now_us = now_epoch_us();
        let limit_i64 = i64::try_from(limit).unwrap_or(i64::MAX);

        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT payload FROM records
                     WHERE project_id = ?1
                       AND tombstone = 0
                       AND version > indexed_version
                       AND (expires_at_us IS NULL OR expires_at_us > ?2)
                     ORDER BY seq ASC
                     LIMIT ?3",
                )
                .map_err(map_sqlite_err)?;
            let rows = stmt
                .query_map(params![ns_str, now_us, limit_i64], |row| {
                    row.get::<_, String>(0)
                })
                .map_err(map_sqlite_err)?;

            let mut items = Vec::new();
            for row in rows {
                let json = row.map_err(map_sqlite_err)?;
                items.push(serde_json::from_str::<MemoryItem>(&json)?);
            }
            Ok(items)
        })
        .await
    }

    async fn session_items(
        &self,
        ns: &ProjectId,
        session: &SessionId,
        limit: usize,
    ) -> Result<Vec<MemoryItem>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let ns_str = ns.as_str().to_string();
        let scope = format!("session:{}", session.as_str());
        let now_us = now_epoch_us();
        let limit_i64 = i64::try_from(limit).unwrap_or(i64::MAX);

        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT payload FROM records
                     WHERE project_id = ?1
                       AND scope = ?2
                       AND tombstone = 0
                       AND (expires_at_us IS NULL OR expires_at_us > ?3)
                     ORDER BY seq DESC
                     LIMIT ?4",
                )
                .map_err(map_sqlite_err)?;
            let rows = stmt
                .query_map(params![ns_str, scope, now_us, limit_i64], |row| {
                    row.get::<_, String>(0)
                })
                .map_err(map_sqlite_err)?;

            let mut items = Vec::new();
            for row in rows {
                let json = row.map_err(map_sqlite_err)?;
                items.push(serde_json::from_str::<MemoryItem>(&json)?);
            }
            Ok(items)
        })
        .await
    }

    async fn mark_indexed(&self, ns: &ProjectId, id: MemoryId, version: u64) -> Result<()> {
        let ns_str = ns.as_str().to_string();
        let id_str = id.to_string();
        let ver_i64 = version as i64;

        self.with_conn(move |conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(map_sqlite_err)?;

            tx.execute(
                "UPDATE records
                 SET indexed_version = ?1
                 WHERE project_id = ?2 AND memory_id = ?3 AND version = ?1",
                params![ver_i64, ns_str, id_str],
            )
            .map_err(map_sqlite_err)?;

            tx.execute(
                "DELETE FROM records
                 WHERE project_id = ?1
                   AND memory_id = ?2
                   AND tombstone = 1
                   AND version <= indexed_version",
                params![ns_str, id_str],
            )
            .map_err(map_sqlite_err)?;

            tx.commit().map_err(map_sqlite_err)?;
            Ok(())
        })
        .await
    }
}

fn next_seq(tx: &Transaction<'_>) -> rusqlite::Result<i64> {
    tx.query_row(
        "UPDATE seq_counter SET val = val + 1 WHERE id = 1 RETURNING val",
        [],
        |row| row.get(0),
    )
}

fn now_epoch_us() -> i64 {
    let micros = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros();
    i64::try_from(micros).unwrap_or(i64::MAX)
}

fn expires_at_us(now_us: i64, ttl: Option<Duration>) -> Option<i64> {
    ttl.map(|d| {
        let delta = i64::try_from(d.as_micros()).unwrap_or(i64::MAX);
        now_us.saturating_add(delta)
    })
}

fn scope_as_str(scope: &MemoryScope) -> String {
    match scope {
        MemoryScope::Project => "project".into(),
        MemoryScope::User(u) => format!("user:{}", u.as_str()),
        MemoryScope::Session(s) => format!("session:{}", s.as_str()),
    }
}

fn permanent_err(message: impl Into<String>) -> AetherError {
    AetherError::Permanent {
        connector: CONNECTOR_NAME,
        message: message.into(),
    }
}

fn map_sqlite_err(err: rusqlite::Error) -> AetherError {
    if let rusqlite::Error::SqliteFailure(sqlite_err, _) = &err {
        if matches!(
            sqlite_err.code,
            ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked
        ) {
            return AetherError::Transient {
                connector: CONNECTOR_NAME,
                message: err.to_string(),
            };
        }
    }
    permanent_err(err.to_string())
}
