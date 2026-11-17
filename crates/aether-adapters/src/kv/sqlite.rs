//! SQLite WAL-mode `RecordStore` connector.

use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aether_core::{
    AetherError, ConnectorCapabilities, ConnectorHealth, MemoryId, MemoryItem, MemoryReceipt,
    MemoryScope, OutboundConnector, ProjectId, RecordStore, Result,
};
use async_trait::async_trait;
use rusqlite::{
    params, Connection, ErrorCode, OptionalExtension, Transaction, TransactionBehavior,
};

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
    ON records (seq DESC);

CREATE TABLE IF NOT EXISTS idempotency_keys (
    project_id TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    expires_at_us INTEGER,
    receipt_json TEXT NOT NULL,
    PRIMARY KEY (project_id, idempotency_key)
);
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

    async fn put(&self, ns: &ProjectId, item: &MemoryItem, ttl: Option<Duration>) -> Result<()> {
        let ns_str = ns.as_str().to_string();
        let id_str = item.id.to_string();
        let scope = scope_as_str(item.scope);
        let version = item.version as i64;
        let payload = serde_json::to_string(item)?;
        let now_us = now_epoch_us();
        let exp_us = expires_at_us(now_us, ttl);

        self.with_conn(move |conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(map_sqlite_err)?;
            let seq = next_seq(&tx).map_err(map_sqlite_err)?;
            tx.execute(
                "INSERT INTO records (project_id, memory_id, scope, version, seq, expires_at_us, payload)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(project_id, memory_id) DO UPDATE SET
                     scope = excluded.scope,
                     version = excluded.version,
                     seq = excluded.seq,
                     expires_at_us = excluded.expires_at_us,
                     payload = excluded.payload",
                params![ns_str, id_str, scope, version, seq, exp_us, payload],
            )
            .map_err(map_sqlite_err)?;
            tx.commit().map_err(map_sqlite_err)?;
            Ok(())
        })
        .await
    }

    async fn put_if_absent(
        &self,
        ns: &ProjectId,
        idempotency_key: &str,
        receipt: &MemoryReceipt,
        ttl: Option<Duration>,
    ) -> Result<Option<MemoryReceipt>> {
        let ns_str = ns.as_str().to_string();
        let key = idempotency_key.to_string();
        let receipt_json = serde_json::to_string(receipt)?;
        let now_us = now_epoch_us();
        let exp_us = expires_at_us(now_us, ttl);

        self.with_conn(move |conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(map_sqlite_err)?;

            let existing: Option<(String, Option<i64>)> = tx
                .query_row(
                    "SELECT receipt_json, expires_at_us
                     FROM idempotency_keys
                     WHERE project_id = ?1 AND idempotency_key = ?2",
                    params![ns_str, key],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(map_sqlite_err)?;

            if let Some((json, existing_exp)) = existing {
                if existing_exp.is_none_or(|exp| exp > now_us) {
                    let existing_receipt = serde_json::from_str::<MemoryReceipt>(&json)?;
                    return Ok(Some(existing_receipt));
                }
            }

            tx.execute(
                "INSERT INTO idempotency_keys (project_id, idempotency_key, expires_at_us, receipt_json)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(project_id, idempotency_key) DO UPDATE SET
                     expires_at_us = excluded.expires_at_us,
                     receipt_json = excluded.receipt_json",
                params![ns_str, key, exp_us, receipt_json],
            )
            .map_err(map_sqlite_err)?;
            tx.commit().map_err(map_sqlite_err)?;
            Ok(None)
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
        let scope = scope_as_str(new_item.scope);
        let new_version = new_item.version as i64;
        let payload = serde_json::to_string(new_item)?;
        let now_us = now_epoch_us();

        self.with_conn(move |conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(map_sqlite_err)?;

            let current: Option<(i64, Option<i64>)> = tx
                .query_row(
                    "SELECT version, expires_at_us
                     FROM records
                     WHERE project_id = ?1 AND memory_id = ?2",
                    params![ns_str, id_str],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(map_sqlite_err)?;

            let Some((actual_version_i64, existing_exp)) = current else {
                return Err(AetherError::NotFound(format!("memory {id}")));
            };
            if existing_exp.is_some_and(|exp| exp <= now_us) {
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

    async fn recent(
        &self,
        ns: &ProjectId,
        scope: Option<MemoryScope>,
        limit: usize,
    ) -> Result<Vec<MemoryItem>> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let ns_str = ns.as_str().to_string();
        let now_us = now_epoch_us();
        let limit_i64 = i64::try_from(limit).unwrap_or(i64::MAX);
        let scope_filter = scope.map(scope_as_str);

        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT payload FROM records
                     WHERE project_id = ?1
                       AND (?2 IS NULL OR scope = ?2)
                       AND (expires_at_us IS NULL OR expires_at_us > ?3)
                     ORDER BY seq DESC
                     LIMIT ?4",
                )
                .map_err(map_sqlite_err)?;
            let rows = stmt
                .query_map(params![ns_str, scope_filter, now_us, limit_i64], |row| {
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

    async fn delete(&self, ns: &ProjectId, id: MemoryId) -> Result<()> {
        let ns_str = ns.as_str().to_string();
        let id_str = id.to_string();

        self.with_conn(move |conn| {
            conn.execute(
                "DELETE FROM records WHERE project_id = ?1 AND memory_id = ?2",
                params![ns_str, id_str],
            )
            .map_err(map_sqlite_err)?;
            Ok(())
        })
        .await
    }
}

fn next_seq(tx: &Transaction<'_>) -> rusqlite::Result<i64> {
    tx.query_row("SELECT COALESCE(MAX(seq), 0) + 1 FROM records", [], |row| {
        row.get(0)
    })
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

fn scope_as_str(scope: MemoryScope) -> &'static str {
    match scope {
        MemoryScope::Project => "project",
        MemoryScope::User => "user",
        MemoryScope::Session => "session",
        MemoryScope::Global => "global",
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
