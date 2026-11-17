//! Redis / Valkey `RecordStore` connector.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aether_core::{
    redact_url_userinfo, AetherError, ConnectorCapabilities, ConnectorHealth, MemoryId, MemoryItem,
    MemoryReceipt, MemoryScope, OutboundConnector, ProjectId, RecordStore, Result, SecretString,
};
use async_trait::async_trait;
use redis::aio::ConnectionManager;
use redis::{AsyncCommands, ErrorKind, ExistenceCheck, RedisError, Script, SetExpiry, SetOptions};

const CONNECTOR_NAME: &str = "redis";
const MAX_BATCH_SIZE: usize = 256;

const CAS_UPDATE_LUA: &str = r#"
local raw = redis.call('GET', KEYS[1])
if not raw then
    return {'NOT_FOUND'}
end
local ok, doc = pcall(cjson.decode, raw)
if not ok or type(doc) ~= 'table' or doc['version'] == nil then
    return {'CORRUPT', 'stored record is missing valid JSON version'}
end
local actual = doc['version']
if ARGV[1] ~= '' then
    local expected = tonumber(ARGV[1])
    if actual ~= expected then
        return {'CONFLICT', string.format('%.0f', actual)}
    end
end
local pttl = redis.call('PTTL', KEYS[1])
if pttl > 0 then
    redis.call('SET', KEYS[1], ARGV[2], 'PX', pttl)
else
    redis.call('SET', KEYS[1], ARGV[2])
end
if KEYS[2] and ARGV[3] ~= '' then
    redis.call('ZADD', KEYS[2], tonumber(ARGV[3]), ARGV[4])
end
return {'OK'}
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
    cas_script: Script,
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
            cas_script: Script::new(CAS_UPDATE_LUA),
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
    async fn get(&self, ns: &ProjectId, id: MemoryId) -> Result<Option<MemoryItem>> {
        let mut conn = self.manager.clone();
        let key = self.keys.record_key(ns, id);
        let raw: Option<String> = conn.get(&key).await.map_err(map_redis_error)?;
        raw.map(|s| serde_json::from_str::<MemoryItem>(&s))
            .transpose()
            .map_err(AetherError::from)
    }

    async fn put(&self, ns: &ProjectId, item: &MemoryItem, ttl: Option<Duration>) -> Result<()> {
        let mut conn = self.manager.clone();
        let key = self.keys.record_key(ns, item.id);
        let recent_key = self.keys.recent_index_key(ns);
        let id_str = item.id.to_string();

        if matches!(ttl, Some(d) if d.is_zero()) {
            let mut pipe = redis::pipe();
            pipe.atomic().del(&key).zrem(&recent_key, &id_str);
            pipe.query_async::<()>(&mut conn)
                .await
                .map_err(map_redis_error)?;
            return Ok(());
        }

        let payload = serde_json::to_string(item)?;
        let score = self.next_score();

        let mut set_opts = SetOptions::default();
        if let Some(d) = ttl {
            set_opts = set_opts.with_expiration(SetExpiry::PX(ttl_millis(d)));
        }

        let mut pipe = redis::pipe();
        pipe.atomic()
            .cmd("SET")
            .arg(&key)
            .arg(&payload)
            .arg(set_opts)
            .ignore()
            .zadd(&recent_key, &id_str, score)
            .ignore();
        pipe.query_async::<()>(&mut conn)
            .await
            .map_err(map_redis_error)?;
        Ok(())
    }

    async fn put_if_absent(
        &self,
        ns: &ProjectId,
        idempotency_key: &str,
        receipt: &MemoryReceipt,
        ttl: Option<Duration>,
    ) -> Result<Option<MemoryReceipt>> {
        if matches!(ttl, Some(d) if d.is_zero()) {
            return Ok(None);
        }

        let mut conn = self.manager.clone();
        let key = self.keys.idempotency_key(ns, idempotency_key);
        let payload = serde_json::to_string(receipt)?;

        let mut opts = SetOptions::default()
            .conditional_set(ExistenceCheck::NX)
            .get(true);
        if let Some(d) = ttl {
            opts = opts.with_expiration(SetExpiry::PX(ttl_millis(d)));
        }

        let existing: Option<String> = conn
            .set_options(&key, &payload, opts)
            .await
            .map_err(map_redis_error)?;

        existing
            .map(|raw| serde_json::from_str::<MemoryReceipt>(&raw))
            .transpose()
            .map_err(AetherError::from)
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
        let recent_key = self.keys.recent_index_key(ns);
        let expected_arg = expected_version.map_or_else(String::new, |v| v.to_string());
        let payload = serde_json::to_string(new_item)?;
        let score = self.next_score();
        let id_str = id.to_string();

        let reply: Vec<String> = self
            .cas_script
            .key(&key)
            .key(&recent_key)
            .arg(&expected_arg)
            .arg(&payload)
            .arg(score)
            .arg(&id_str)
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

    async fn recent(
        &self,
        ns: &ProjectId,
        scope: Option<MemoryScope>,
        limit: usize,
    ) -> Result<Vec<MemoryItem>> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let mut conn = self.manager.clone();
        let recent_key = self.keys.recent_index_key(ns);
        let batch_size = if scope.is_some() {
            limit.saturating_mul(4).clamp(64, MAX_BATCH_SIZE)
        } else {
            limit.saturating_mul(2).clamp(32, MAX_BATCH_SIZE)
        };

        let mut results = Vec::with_capacity(limit);
        let mut stale_ids = Vec::new();
        let mut offset: isize = 0;

        loop {
            let stop = offset + (batch_size as isize) - 1;
            let ids: Vec<String> = conn
                .zrevrange(&recent_key, offset, stop)
                .await
                .map_err(map_redis_error)?;
            if ids.is_empty() {
                break;
            }

            let fetched_len = ids.len();
            let rec_keys: Vec<String> = ids
                .iter()
                .map(|raw_id| self.keys.record_key_from_raw_id(ns, raw_id))
                .collect();
            let values: Vec<Option<String>> =
                conn.mget(&rec_keys).await.map_err(map_redis_error)?;

            for (raw_id, maybe_raw) in ids.into_iter().zip(values) {
                match maybe_raw {
                    Some(raw) => {
                        let item: MemoryItem = serde_json::from_str(&raw)?;
                        if scope.is_none_or(|s| item.scope == s) {
                            results.push(item);
                            if results.len() == limit {
                                break;
                            }
                        }
                    }
                    None => {
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
            conn.zrem::<_, _, ()>(&recent_key, &stale_ids)
                .await
                .map_err(map_redis_error)?;
        }

        Ok(results)
    }

    async fn delete(&self, ns: &ProjectId, id: MemoryId) -> Result<()> {
        let mut conn = self.manager.clone();
        let key = self.keys.record_key(ns, id);
        let recent_key = self.keys.recent_index_key(ns);
        let id_str = id.to_string();

        let mut pipe = redis::pipe();
        pipe.atomic().del(&key).zrem(&recent_key, &id_str);
        pipe.query_async::<()>(&mut conn)
            .await
            .map_err(map_redis_error)?;
        Ok(())
    }
}
