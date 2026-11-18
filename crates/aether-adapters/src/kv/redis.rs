//! Redis / Valkey `RecordStore` connector.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use chrono::Utc;
use nytheris_aether_core::{
    redact_url_userinfo, AetherError, ConnectorCapabilities, ConnectorHealth, CreateOutcome,
    MemoryId, MemoryItem, MemoryReceipt, MemoryScope, OutboundConnector, PendingRecord, ProjectId,
    ReceiptState, RecordStore, Result, SecretString, SessionId,
};
use redis::aio::ConnectionManager;
use redis::{AsyncCommands, ErrorKind, RedisError, Script};
use uuid::Uuid;

const CONNECTOR_NAME: &str = "redis";
const MAX_BATCH_SIZE: usize = 256;
const MAX_TOMBSTONE_RETRIES: usize = 32;

const CREATE_LUA: &str = r#"
local rec_key = KEYS[1]
local proj_pending_key = KEYS[2]
local global_pending_key = KEYS[3]

local id_str = ARGV[1]
local payload = ARGV[2]
local version = ARGV[3]
local seq = ARGV[4]
local px_ms = ARGV[5]
local has_idem = ARGV[6]
local idem_val = ARGV[7]
local rec_key_prefix = ARGV[8]
local idem_key_prefix = ARGV[9]
local has_session = ARGV[10]

local idx = 4
local idem_key = nil
local rec_idem_key = nil
if has_idem == '1' then
    idem_key = KEYS[idx]
    rec_idem_key = KEYS[idx + 1]
    idx = idx + 2
end
local session_key = nil
if has_session == '1' then
    session_key = KEYS[idx]
end

if has_idem == '1' then
    local existing_id = redis.call('GET', idem_key)
    if existing_id then
        local existing_rec_key = rec_key_prefix .. existing_id
        local info = redis.call('HMGET', existing_rec_key, 'version', 'indexed_version', 'tombstone')
        local ver = info[1]
        local idx_ver = info[2]
        local tomb = info[3]
        if ver and tomb ~= '1' then
            return {'DUP', existing_id, tostring(ver), tostring(idx_ver or '0')}
        end
    end
end

if px_ms == '0' then
    redis.call('DEL', rec_key)
    redis.call('ZREM', proj_pending_key, id_str)
    redis.call('ZREM', global_pending_key, rec_key)
    if has_session == '1' then
        redis.call('ZREM', session_key, id_str)
    end
    return {'CREATED'}
end

if has_idem == '1' then
    local prev_idem = redis.call('GET', rec_idem_key)
    if prev_idem and prev_idem ~= idem_val then
        local old_idem_key = idem_key_prefix .. prev_idem
        if redis.call('GET', old_idem_key) == id_str then
            redis.call('DEL', old_idem_key)
        end
    end
end

redis.call('HSET', rec_key, 'payload', payload, 'version', version, 'indexed_version', '0', 'tombstone', '0', 'seq', seq)
if px_ms ~= '' then
    redis.call('PEXPIRE', rec_key, tonumber(px_ms))
else
    redis.call('PERSIST', rec_key)
end

if has_idem == '1' then
    if px_ms ~= '' then
        redis.call('SET', idem_key, id_str, 'PX', tonumber(px_ms))
        redis.call('SET', rec_idem_key, idem_val, 'PX', tonumber(px_ms))
    else
        redis.call('SET', idem_key, id_str)
        redis.call('SET', rec_idem_key, idem_val)
    end
end

local score = tonumber(seq)
redis.call('ZADD', global_pending_key, score, rec_key)
redis.call('ZADD', proj_pending_key, score, id_str)
if has_session == '1' then
    redis.call('ZADD', session_key, score, id_str)
end

return {'CREATED'}
"#;

const CAS_UPDATE_LUA: &str = r#"
local rec_key = KEYS[1]
local proj_pending_key = KEYS[2]
local global_pending_key = KEYS[3]

local expected_arg = ARGV[1]
local payload = ARGV[2]
local new_version = ARGV[3]
local seq = ARGV[4]
local id_str = ARGV[5]
local has_session = ARGV[6]

local info = redis.call('HMGET', rec_key, 'version', 'tombstone')
local cur_ver = info[1]
local tomb = info[2]
if not cur_ver or tomb == '1' then
    return {'NOT_FOUND'}
end

local actual = tonumber(cur_ver)
if not actual then
    return {'CORRUPT', 'stored record is missing valid version'}
end

if expected_arg ~= '' then
    local expected = tonumber(expected_arg)
    if actual ~= expected then
        return {'CONFLICT', string.format('%.0f', actual)}
    end
end

redis.call('HSET', rec_key, 'payload', payload, 'version', new_version, 'seq', seq)

local score = tonumber(seq)
redis.call('ZADD', global_pending_key, score, rec_key)
redis.call('ZADD', proj_pending_key, score, id_str)
if has_session == '1' then
    redis.call('ZADD', KEYS[4], score, id_str)
end
return {'OK'}
"#;

const TOMBSTONE_LUA: &str = r#"
local rec_key = KEYS[1]
local rec_idem_key = KEYS[2]
local proj_pending_key = KEYS[3]
local global_pending_key = KEYS[4]

local expected_ver = tonumber(ARGV[1])
local new_ver = ARGV[2]
local updated_payload = ARGV[3]
local seq = ARGV[4]
local id_str = ARGV[5]
local idem_key_prefix = ARGV[6]
local has_session = ARGV[7]

local info = redis.call('HMGET', rec_key, 'version', 'tombstone')
local cur_ver = info[1]
local tomb = info[2]
if not cur_ver or tomb == '1' then
    return 'NONE'
end

if tonumber(cur_ver) ~= expected_ver then
    return 'RETRY'
end

redis.call('HSET', rec_key, 'version', new_ver, 'tombstone', '1', 'seq', seq, 'payload', updated_payload)

local raw_idem = redis.call('GET', rec_idem_key)
if raw_idem then
    local idem_key = idem_key_prefix .. raw_idem
    if redis.call('GET', idem_key) == id_str then
        redis.call('DEL', idem_key)
    end
    redis.call('DEL', rec_idem_key)
end

local score = tonumber(seq)
redis.call('ZADD', global_pending_key, score, rec_key)
redis.call('ZREM', proj_pending_key, id_str)
if has_session == '1' then
    redis.call('ZREM', KEYS[5], id_str)
end

return 'OK'
"#;

const MARK_INDEXED_LUA: &str = r#"
local rec_key = KEYS[1]
local proj_pending_key = KEYS[2]
local global_pending_key = KEYS[3]
local rec_idem_key = KEYS[4]

local expected_ver = tonumber(ARGV[1])
local id_str = ARGV[2]

local info = redis.call('HMGET', rec_key, 'version', 'tombstone')
local cur_ver = info[1]
local tomb = info[2]
if not cur_ver then
    return 'OK'
end

if tonumber(cur_ver) ~= expected_ver then
    return 'OK'
end

redis.call('ZREM', global_pending_key, rec_key)
redis.call('ZREM', proj_pending_key, id_str)
if tomb == '1' then
    redis.call('DEL', rec_key)
    redis.call('DEL', rec_idem_key)
else
    redis.call('HSET', rec_key, 'indexed_version', ARGV[1])
end

return 'OK'
"#;

/// Connection and key-namespace configuration for [`RedisRecordStore`].
#[derive(Clone)]
pub struct RedisConfig {
    pub url: SecretString,
    pub key_prefix: String,
}

impl RedisConfig {
    /// Creates a new Redis configuration with the given connection URL and key prefix.
    pub fn new(url: impl Into<SecretString>, key_prefix: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            key_prefix: key_prefix.into(),
        }
    }

    /// Validates that `url` and `key_prefix` are non-empty and that `url` parses as a Redis URL.
    pub fn validate(&self) -> Result<()> {
        let raw_url = self.url.expose_secret().trim();
        if raw_url.is_empty() {
            return Err(AetherError::InvalidInput(
                "redis url must not be empty".into(),
            ));
        }
        if self.key_prefix.trim().is_empty() {
            return Err(AetherError::InvalidInput(
                "redis key_prefix must not be empty".into(),
            ));
        }
        redis::Client::open(raw_url)
            .map_err(|err| AetherError::InvalidInput(format!("invalid redis url: {err}")))?;
        Ok(())
    }

    /// Builds a validated [`RedisKeyLayout`] scoped by the configured `key_prefix`.
    pub fn key_layout(&self) -> Result<RedisKeyLayout> {
        let prefix = self.key_prefix.trim().trim_end_matches(':');
        if prefix.is_empty() {
            return Err(AetherError::InvalidInput(
                "redis key_prefix must not be empty".into(),
            ));
        }
        Ok(RedisKeyLayout {
            prefix: prefix.to_owned(),
        })
    }
}

impl fmt::Debug for RedisConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RedisConfig")
            .field("url", &redact_url_userinfo(self.url.expose_secret()))
            .field("key_prefix", &self.key_prefix)
            .finish()
    }
}

/// Deterministic Redis key builder scoped by `key_prefix` and [`ProjectId`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedisKeyLayout {
    prefix: String,
}

impl RedisKeyLayout {
    /// Returns the record key for a `(ProjectId, MemoryId)` pair.
    pub fn record_key(&self, ns: &ProjectId, id: MemoryId) -> String {
        format!("{}:{{{}}}:rec:{id}", self.prefix, ns.as_str())
    }

    /// Returns the idempotency receipt key for a `(ProjectId, idempotency_key)` pair.
    pub fn idempotency_key(&self, ns: &ProjectId, idempotency_key: &str) -> String {
        format!("{}:{{{}}}:idem:{idempotency_key}", self.prefix, ns.as_str())
    }

    /// Returns the per-project sorted-set key tracking recent memory IDs.
    pub fn recent_index_key(&self, ns: &ProjectId) -> String {
        format!("{}:{{{}}}:recent", self.prefix, ns.as_str())
    }

    /// Returns the reverse pointer key mapping a `(ProjectId, MemoryId)` to its idempotency key.
    pub fn record_idem_key(&self, ns: &ProjectId, id: MemoryId) -> String {
        format!("{}:{{{}}}:rec_idem:{id}", self.prefix, ns.as_str())
    }

    /// Returns the global sorted-set key tracking pending records across projects.
    pub fn global_pending_key(&self) -> String {
        format!("{}:pending", self.prefix)
    }

    /// Returns the per-project sorted-set key tracking pending live memory IDs.
    pub fn project_pending_key(&self, ns: &ProjectId) -> String {
        format!("{}:{{{}}}:pending", self.prefix, ns.as_str())
    }

    /// Returns the per-session sorted-set key tracking session-scoped memory IDs.
    pub fn session_index_key(&self, ns: &ProjectId, session: &SessionId) -> String {
        format!(
            "{}:{{{}}}:sess:{}",
            self.prefix,
            ns.as_str(),
            session.as_str()
        )
    }

    fn record_key_prefix(&self, ns: &ProjectId) -> String {
        format!("{}:{{{}}}:rec:", self.prefix, ns.as_str())
    }

    fn idempotency_key_prefix(&self, ns: &ProjectId) -> String {
        format!("{}:{{{}}}:idem:", self.prefix, ns.as_str())
    }

    fn record_key_from_raw_id(&self, ns: &ProjectId, raw_id: &str) -> String {
        format!("{}:{{{}}}:rec:{raw_id}", self.prefix, ns.as_str())
    }
}

/// Maps a [`redis::RedisError`] into a retryable [`AetherError::Transient`] or non-retryable [`AetherError::Permanent`].
pub fn map_redis_error(err: RedisError) -> AetherError {
    let is_transient = err.is_io_error()
        || err.is_timeout()
        || err.is_connection_dropped()
        || matches!(
            err.kind(),
            ErrorKind::IoError
                | ErrorKind::BusyLoadingError
                | ErrorKind::TryAgain
                | ErrorKind::MasterDown
                | ErrorKind::ClusterDown
                | ErrorKind::Moved
                | ErrorKind::Ask
                | ErrorKind::ClusterConnectionNotFound
                | ErrorKind::NoScriptError
                | ErrorKind::MasterNameNotFoundBySentinel
                | ErrorKind::NoValidReplicasFoundBySentinel
        );

    if is_transient {
        AetherError::Transient {
            connector: CONNECTOR_NAME,
            message: err.to_string(),
        }
    } else {
        AetherError::Permanent {
            connector: CONNECTOR_NAME,
            message: err.to_string(),
        }
    }
}

/// Redis / Valkey implementation of [`RecordStore`] backed by [`ConnectionManager`].
pub struct RedisRecordStore {
    config: RedisConfig,
    keys: RedisKeyLayout,
    manager: ConnectionManager,
    last_score: AtomicU64,
    create_script: Script,
    cas_script: Script,
    tombstone_script: Script,
    mark_indexed_script: Script,
}

impl fmt::Debug for RedisRecordStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RedisRecordStore")
            .field("config", &self.config)
            .field("keys", &self.keys)
            .finish_non_exhaustive()
    }
}

impl RedisRecordStore {
    /// Validates configuration and establishes a multiplexing [`ConnectionManager`].
    pub async fn connect(config: RedisConfig) -> Result<Self> {
        config.validate()?;
        let keys = config.key_layout()?;
        let client = redis::Client::open(config.url.expose_secret().trim())
            .map_err(|err| AetherError::InvalidInput(format!("invalid redis url: {err}")))?;
        let manager = ConnectionManager::new(client)
            .await
            .map_err(map_redis_error)?;

        Ok(Self {
            config,
            keys,
            manager,
            last_score: AtomicU64::new(0),
            create_script: Script::new(CREATE_LUA),
            cas_script: Script::new(CAS_UPDATE_LUA),
            tombstone_script: Script::new(TOMBSTONE_LUA),
            mark_indexed_script: Script::new(MARK_INDEXED_LUA),
        })
    }

    /// Returns the store configuration (with redacted credentials in `Debug`).
    pub fn config(&self) -> &RedisConfig {
        &self.config
    }

    /// Generates a monotonically increasing score encoded safely within `f64`'s 53-bit integer range.
    fn next_score(&self) -> f64 {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        // Shifting by 10 bits reserves 1024 sub-millisecond slots while staying exact in IEEE-754 f64.
        let floor = now_ms.saturating_mul(1024);
        let mut current = self.last_score.load(Ordering::Relaxed);
        loop {
            let next = if current >= floor { current + 1 } else { floor };
            match self.last_score.compare_exchange_weak(
                current,
                next,
                Ordering::SeqCst,
                Ordering::Relaxed,
            ) {
                Ok(_) => return next as f64,
                Err(observed) => current = observed,
            }
        }
    }
}

fn ttl_millis(ttl: Duration) -> u64 {
    u64::try_from(ttl.as_millis()).unwrap_or(u64::MAX).max(1)
}

#[async_trait]
impl OutboundConnector for RedisRecordStore {
    fn name(&self) -> &'static str {
        CONNECTOR_NAME
    }

    fn capabilities(&self) -> ConnectorCapabilities {
        ConnectorCapabilities {
            max_batch_size: MAX_BATCH_SIZE,
            supports_temporal: false,
            supports_filter_expr: false,
        }
    }

    async fn health_check(&self) -> Result<ConnectorHealth> {
        let mut conn = self.manager.clone();
        let reply: String = redis::cmd("PING")
            .query_async(&mut conn)
            .await
            .map_err(map_redis_error)?;
        if reply.eq_ignore_ascii_case("PONG") {
            Ok(ConnectorHealth::Healthy)
        } else {
            Ok(ConnectorHealth::Degraded(format!(
                "unexpected PING reply: {reply}"
            )))
        }
    }

    async fn close(&self) -> Result<()> {
        Ok(())
    }
}

#[async_trait]
impl RecordStore for RedisRecordStore {
    async fn create(
        &self,
        ns: &ProjectId,
        item: &MemoryItem,
        ttl: Option<Duration>,
    ) -> Result<CreateOutcome> {
        let mut conn = self.manager.clone();
        let rec_key = self.keys.record_key(ns, item.id);
        let proj_pending_key = self.keys.project_pending_key(ns);
        let global_pending_key = self.keys.global_pending_key();
        let id_str = item.id.to_string();
        let payload = serde_json::to_string(item)?;
        let version_str = item.version.to_string();
        let score = self.next_score();
        let px_ms = match ttl {
            None => String::new(),
            Some(d) if d.is_zero() => "0".to_string(),
            Some(d) => ttl_millis(d).to_string(),
        };

        let mut inv = self.create_script.prepare_invoke();
        inv.key(&rec_key)
            .key(&proj_pending_key)
            .key(&global_pending_key);

        let (has_idem, idem_val) = match &item.idempotency_key {
            Some(k) => {
                inv.key(self.keys.idempotency_key(ns, k))
                    .key(self.keys.record_idem_key(ns, item.id));
                ("1", k.as_str())
            }
            None => ("0", ""),
        };

        let has_session = match &item.scope {
            MemoryScope::Session(session) => {
                inv.key(self.keys.session_index_key(ns, session));
                "1"
            }
            _ => "0",
        };

        let rec_key_prefix = self.keys.record_key_prefix(ns);
        let idem_key_prefix = self.keys.idempotency_key_prefix(ns);

        let reply: Vec<String> = inv
            .arg(&id_str)
            .arg(&payload)
            .arg(&version_str)
            .arg(score)
            .arg(&px_ms)
            .arg(has_idem)
            .arg(idem_val)
            .arg(&rec_key_prefix)
            .arg(&idem_key_prefix)
            .arg(has_session)
            .invoke_async(&mut conn)
            .await
            .map_err(map_redis_error)?;

        match reply.first().map(String::as_str) {
            Some("CREATED") => Ok(CreateOutcome::Created),
            Some("DUP") => {
                let raw_id = reply.get(1).ok_or_else(|| AetherError::Permanent {
                    connector: CONNECTOR_NAME,
                    message: format!("missing memory_id in DUP reply: {reply:?}"),
                })?;
                let uuid = Uuid::parse_str(raw_id).map_err(|err| AetherError::Permanent {
                    connector: CONNECTOR_NAME,
                    message: format!("corrupt memory_id in redis: {err}"),
                })?;
                let version = reply
                    .get(2)
                    .and_then(|s| s.parse::<u64>().ok())
                    .ok_or_else(|| AetherError::Permanent {
                        connector: CONNECTOR_NAME,
                        message: format!("invalid version in DUP reply: {reply:?}"),
                    })?;
                let indexed_version = reply
                    .get(3)
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(0);
                let state = if indexed_version >= version {
                    ReceiptState::Persisted
                } else {
                    ReceiptState::Buffered
                };
                Ok(CreateOutcome::Duplicate(MemoryReceipt {
                    memory_id: MemoryId::from_uuid(uuid),
                    state,
                    version,
                }))
            }
            _ => Err(AetherError::Permanent {
                connector: CONNECTOR_NAME,
                message: format!("unexpected CREATE script response: {reply:?}"),
            }),
        }
    }

    async fn get(&self, ns: &ProjectId, id: MemoryId) -> Result<Option<MemoryItem>> {
        let mut conn = self.manager.clone();
        let key = self.keys.record_key(ns, id);
        let row: Vec<Option<String>> = redis::cmd("HMGET")
            .arg(&key)
            .arg("tombstone")
            .arg("payload")
            .query_async(&mut conn)
            .await
            .map_err(map_redis_error)?;

        let tomb = row.first().and_then(|v| v.as_deref());
        let payload = row.get(1).and_then(|v| v.as_deref());
        if tomb == Some("1") {
            return Ok(None);
        }
        payload
            .map(serde_json::from_str::<MemoryItem>)
            .transpose()
            .map_err(AetherError::from)
    }

    async fn get_many(&self, ns: &ProjectId, ids: &[MemoryId]) -> Result<Vec<MemoryItem>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }

        let mut conn = self.manager.clone();
        let mut items = Vec::with_capacity(ids.len());
        for chunk in ids.chunks(MAX_BATCH_SIZE) {
            let mut pipe = redis::pipe();
            for &id in chunk {
                let key = self.keys.record_key(ns, id);
                pipe.cmd("HMGET").arg(key).arg("tombstone").arg("payload");
            }
            let rows: Vec<Vec<Option<String>>> =
                pipe.query_async(&mut conn).await.map_err(map_redis_error)?;
            for row in rows {
                let tomb = row.first().and_then(|v| v.as_deref());
                let payload = row.get(1).and_then(|v| v.as_deref());
                if tomb == Some("1") {
                    continue;
                }
                if let Some(raw) = payload {
                    items.push(serde_json::from_str::<MemoryItem>(raw)?);
                }
            }
        }
        Ok(items)
    }

    async fn cas_update(
        &self,
        ns: &ProjectId,
        id: MemoryId,
        expected_version: Option<u64>,
        new_item: &MemoryItem,
    ) -> Result<()> {
        let mut conn = self.manager.clone();
        let key = self.keys.record_key(ns, id);
        let proj_pending_key = self.keys.project_pending_key(ns);
        let global_pending_key = self.keys.global_pending_key();
        let expected_arg = expected_version.map_or_else(String::new, |v| v.to_string());
        let payload = serde_json::to_string(new_item)?;
        let new_version_str = new_item.version.to_string();
        let score = self.next_score();
        let id_str = id.to_string();

        let mut inv = self.cas_script.prepare_invoke();
        inv.key(&key)
            .key(&proj_pending_key)
            .key(&global_pending_key);
        let has_session = match &new_item.scope {
            MemoryScope::Session(session) => {
                inv.key(self.keys.session_index_key(ns, session));
                "1"
            }
            _ => "0",
        };

        let reply: Vec<String> = inv
            .arg(&expected_arg)
            .arg(&payload)
            .arg(&new_version_str)
            .arg(score)
            .arg(&id_str)
            .arg(has_session)
            .invoke_async(&mut conn)
            .await
            .map_err(map_redis_error)?;

        match reply.first().map(String::as_str) {
            Some("OK") => Ok(()),
            Some("NOT_FOUND") => Err(AetherError::NotFound(format!("memory {id}"))),
            Some("CONFLICT") => {
                let actual_version = reply
                    .get(1)
                    .and_then(|s| s.parse::<u64>().ok())
                    .ok_or_else(|| AetherError::Permanent {
                        connector: CONNECTOR_NAME,
                        message: format!(
                            "invalid conflict version returned by CAS script: {reply:?}"
                        ),
                    })?;
                Err(AetherError::Conflict {
                    memory_id: id,
                    expected_version: expected_version.unwrap_or(0),
                    actual_version,
                })
            }
            Some("CORRUPT") => Err(AetherError::Permanent {
                connector: CONNECTOR_NAME,
                message: reply
                    .get(1)
                    .cloned()
                    .unwrap_or_else(|| "corrupt record in redis".into()),
            }),
            _ => Err(AetherError::Permanent {
                connector: CONNECTOR_NAME,
                message: format!("unexpected CAS script response: {reply:?}"),
            }),
        }
    }

    async fn tombstone(&self, ns: &ProjectId, id: MemoryId) -> Result<Option<MemoryItem>> {
        let mut conn = self.manager.clone();
        let rec_key = self.keys.record_key(ns, id);
        let rec_idem_key = self.keys.record_idem_key(ns, id);
        let proj_pending_key = self.keys.project_pending_key(ns);
        let global_pending_key = self.keys.global_pending_key();
        let id_str = id.to_string();
        let idem_key_prefix = self.keys.idempotency_key_prefix(ns);

        for _ in 0..MAX_TOMBSTONE_RETRIES {
            let row: Vec<Option<String>> = redis::cmd("HMGET")
                .arg(&rec_key)
                .arg("version")
                .arg("tombstone")
                .arg("payload")
                .query_async(&mut conn)
                .await
                .map_err(map_redis_error)?;

            let ver_str = row.first().and_then(|v| v.as_deref());
            let tomb = row.get(1).and_then(|v| v.as_deref());
            let payload_str = row.get(2).and_then(|v| v.as_deref());

            let (Some(ver_str), Some(payload_str)) = (ver_str, payload_str) else {
                return Ok(None);
            };
            if tomb == Some("1") {
                return Ok(None);
            }

            let old_version = ver_str
                .parse::<u64>()
                .map_err(|err| AetherError::Permanent {
                    connector: CONNECTOR_NAME,
                    message: format!("corrupt version in redis: {err}"),
                })?;
            let mut item: MemoryItem = serde_json::from_str(payload_str)?;
            let new_version = old_version.saturating_add(1);
            item.version = new_version;
            item.updated_at = Utc::now();
            let updated_payload = serde_json::to_string(&item)?;
            let score = self.next_score();

            let mut inv = self.tombstone_script.prepare_invoke();
            inv.key(&rec_key)
                .key(&rec_idem_key)
                .key(&proj_pending_key)
                .key(&global_pending_key);
            let has_session = match &item.scope {
                MemoryScope::Session(session) => {
                    inv.key(self.keys.session_index_key(ns, session));
                    "1"
                }
                _ => "0",
            };

            let reply: String = inv
                .arg(old_version)
                .arg(new_version)
                .arg(&updated_payload)
                .arg(score)
                .arg(&id_str)
                .arg(&idem_key_prefix)
                .arg(has_session)
                .invoke_async(&mut conn)
                .await
                .map_err(map_redis_error)?;

            match reply.as_str() {
                "OK" => return Ok(Some(item)),
                "NONE" => return Ok(None),
                "RETRY" => continue,
                _ => {
                    return Err(AetherError::Permanent {
                        connector: CONNECTOR_NAME,
                        message: format!("unexpected TOMBSTONE script response: {reply}"),
                    })
                }
            }
        }

        Err(AetherError::Transient {
            connector: CONNECTOR_NAME,
            message: format!("tombstone exceeded retry limit for memory {id}"),
        })
    }

    async fn pending(&self, limit: usize) -> Result<Vec<PendingRecord>> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let mut conn = self.manager.clone();
        let global_pending_key = self.keys.global_pending_key();
        let batch_size = limit.saturating_mul(2).clamp(32, MAX_BATCH_SIZE);

        let mut results = Vec::with_capacity(limit);
        let mut stale_members = Vec::new();
        let mut offset: isize = 0;

        loop {
            let stop = offset + (batch_size as isize) - 1;
            let members: Vec<String> = conn
                .zrange(&global_pending_key, offset, stop)
                .await
                .map_err(map_redis_error)?;
            if members.is_empty() {
                break;
            }

            let fetched_len = members.len();
            let mut pipe = redis::pipe();
            for rec_key in &members {
                pipe.cmd("HMGET")
                    .arg(rec_key)
                    .arg("version")
                    .arg("indexed_version")
                    .arg("tombstone")
                    .arg("payload");
            }
            let rows: Vec<Vec<Option<String>>> =
                pipe.query_async(&mut conn).await.map_err(map_redis_error)?;

            for (member, row) in members.into_iter().zip(rows) {
                let ver_opt = row
                    .first()
                    .and_then(|v| v.as_deref())
                    .and_then(|s| s.parse::<u64>().ok());
                let idx_ver = row
                    .get(1)
                    .and_then(|v| v.as_deref())
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(0);
                let tomb = row.get(2).and_then(|v| v.as_deref()) == Some("1");
                let payload_opt = row.get(3).and_then(|v| v.as_deref());

                match (ver_opt, payload_opt) {
                    (Some(ver), Some(payload)) if ver > idx_ver => {
                        let item: MemoryItem = serde_json::from_str(payload)?;
                        results.push(PendingRecord {
                            item,
                            tombstone: tomb,
                        });
                        if results.len() == limit {
                            break;
                        }
                    }
                    _ => {
                        stale_members.push(member);
                    }
                }
            }

            if results.len() == limit || fetched_len < batch_size {
                break;
            }
            offset += fetched_len as isize;
        }

        if !stale_members.is_empty() {
            conn.zrem::<_, _, ()>(&global_pending_key, &stale_members)
                .await
                .map_err(map_redis_error)?;
        }

        Ok(results)
    }

    async fn pending_for(&self, ns: &ProjectId, limit: usize) -> Result<Vec<MemoryItem>> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let mut conn = self.manager.clone();
        let proj_pending_key = self.keys.project_pending_key(ns);
        let batch_size = limit.saturating_mul(2).clamp(32, MAX_BATCH_SIZE);

        let mut results = Vec::with_capacity(limit);
        let mut stale_ids = Vec::new();
        let mut offset: isize = 0;

        loop {
            let stop = offset + (batch_size as isize) - 1;
            let ids: Vec<String> = conn
                .zrange(&proj_pending_key, offset, stop)
                .await
                .map_err(map_redis_error)?;
            if ids.is_empty() {
                break;
            }

            let fetched_len = ids.len();
            let mut pipe = redis::pipe();
            for raw_id in &ids {
                let rec_key = self.keys.record_key_from_raw_id(ns, raw_id);
                pipe.cmd("HMGET")
                    .arg(rec_key)
                    .arg("version")
                    .arg("indexed_version")
                    .arg("tombstone")
                    .arg("payload");
            }
            let rows: Vec<Vec<Option<String>>> =
                pipe.query_async(&mut conn).await.map_err(map_redis_error)?;

            for (raw_id, row) in ids.into_iter().zip(rows) {
                let ver_opt = row
                    .first()
                    .and_then(|v| v.as_deref())
                    .and_then(|s| s.parse::<u64>().ok());
                let idx_ver = row
                    .get(1)
                    .and_then(|v| v.as_deref())
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(0);
                let tomb = row.get(2).and_then(|v| v.as_deref()) == Some("1");
                let payload_opt = row.get(3).and_then(|v| v.as_deref());

                match (ver_opt, payload_opt) {
                    (Some(ver), Some(payload)) if !tomb && ver > idx_ver => {
                        results.push(serde_json::from_str::<MemoryItem>(payload)?);
                        if results.len() == limit {
                            break;
                        }
                    }
                    _ => {
                        stale_ids.push(raw_id);
                    }
                }
            }

            if results.len() == limit || fetched_len < batch_size {
                break;
            }
            offset += fetched_len as isize;
        }

        if !stale_ids.is_empty() {
            conn.zrem::<_, _, ()>(&proj_pending_key, &stale_ids)
                .await
                .map_err(map_redis_error)?;
        }

        Ok(results)
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

        let mut conn = self.manager.clone();
        let session_key = self.keys.session_index_key(ns, session);
        let batch_size = limit.saturating_mul(2).clamp(32, MAX_BATCH_SIZE);

        let mut results = Vec::with_capacity(limit);
        let mut stale_ids = Vec::new();
        let mut offset: isize = 0;

        loop {
            let stop = offset + (batch_size as isize) - 1;
            let ids: Vec<String> = conn
                .zrevrange(&session_key, offset, stop)
                .await
                .map_err(map_redis_error)?;
            if ids.is_empty() {
                break;
            }

            let fetched_len = ids.len();
            let mut pipe = redis::pipe();
            for raw_id in &ids {
                let rec_key = self.keys.record_key_from_raw_id(ns, raw_id);
                pipe.cmd("HMGET")
                    .arg(rec_key)
                    .arg("tombstone")
                    .arg("payload");
            }
            let rows: Vec<Vec<Option<String>>> =
                pipe.query_async(&mut conn).await.map_err(map_redis_error)?;

            for (raw_id, row) in ids.into_iter().zip(rows) {
                let tomb = row.first().and_then(|v| v.as_deref()) == Some("1");
                let payload_opt = row.get(1).and_then(|v| v.as_deref());

                match payload_opt {
                    Some(payload) if !tomb => {
                        let item: MemoryItem = serde_json::from_str(payload)?;
                        if matches!(&item.scope, MemoryScope::Session(s) if s == session) {
                            results.push(item);
                            if results.len() == limit {
                                break;
                            }
                        } else {
                            stale_ids.push(raw_id);
                        }
                    }
                    _ => {
                        stale_ids.push(raw_id);
                    }
                }
            }

            if results.len() == limit || fetched_len < batch_size {
                break;
            }
            offset += fetched_len as isize;
        }

        if !stale_ids.is_empty() {
            conn.zrem::<_, _, ()>(&session_key, &stale_ids)
                .await
                .map_err(map_redis_error)?;
        }

        Ok(results)
    }

    async fn mark_indexed(&self, ns: &ProjectId, id: MemoryId, version: u64) -> Result<()> {
        let mut conn = self.manager.clone();
        let rec_key = self.keys.record_key(ns, id);
        let proj_pending_key = self.keys.project_pending_key(ns);
        let global_pending_key = self.keys.global_pending_key();
        let rec_idem_key = self.keys.record_idem_key(ns, id);
        let version_str = version.to_string();
        let id_str = id.to_string();

        let _reply: String = self
            .mark_indexed_script
            .key(&rec_key)
            .key(&proj_pending_key)
            .key(&global_pending_key)
            .key(&rec_idem_key)
            .arg(&version_str)
            .arg(&id_str)
            .invoke_async(&mut conn)
            .await
            .map_err(map_redis_error)?;
        Ok(())
    }
}
