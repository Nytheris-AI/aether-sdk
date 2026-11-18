use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use std::time::Duration;

use chrono::{DateTime, Utc};
use nytheris_aether_core::{AetherError, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::prefix::format_recall_amu_line;
use crate::types::{estimate_tokens, Amu, AmuId, AmuStatus, ContentHash, ScopeTag, VaultTier};

pub const DEFAULT_KV_BLOCK_TOKENS: usize = 16;

pub type KvFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Validated non-empty model identifier (e.g. `Qwen/Qwen2.5-32B-Instruct`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ModelId(String);

impl ModelId {
    pub fn new(raw: impl Into<String>) -> Result<Self> {
        let s = raw.into();
        let trimmed = s.trim();
        if trimmed.is_empty() || trimmed.chars().any(|c| c.is_control()) {
            return Err(AetherError::InvalidInput(format!("invalid model_id `{s}`")));
        }
        Ok(Self(trimmed.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ModelId {
    type Error = AetherError;

    fn try_from(value: String) -> Result<Self> {
        Self::new(value)
    }
}

impl From<ModelId> for String {
    fn from(value: ModelId) -> Self {
        value.0
    }
}

impl fmt::Display for ModelId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Validated 64-hex KV-cache block identifier distinct from file `ContentHash`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct KvBlockHash(ContentHash);

impl KvBlockHash {
    pub fn genesis() -> Self {
        Self(ContentHash::parse(&"0".repeat(64)).expect("64 zeros is valid hex"))
    }

    /// PagedAttention chained prefix block hash:
    /// `SHA-256(model_id || 0x00 || parent_block_hash || 0x00 || block_text)`.
    pub fn paged_chained(model_id: &ModelId, parent: &KvBlockHash, block_text: &str) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(model_id.as_str().as_bytes());
        hasher.update([0]);
        hasher.update(parent.as_str().as_bytes());
        hasher.update([0]);
        hasher.update(block_text.as_bytes());
        let digest: [u8; 32] = hasher.finalize().into();
        Self(ContentHash::parse(&hex::encode(digest)).expect("sha256 hex"))
    }

    /// CacheBlend modular position-independent chunk hash:
    /// `SHA-256(model_id || 0x00 || amu_id || 0x00 || content_hash)`.
    pub fn cache_blend(model_id: &ModelId, amu_id: AmuId, content_hash: &ContentHash) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(model_id.as_str().as_bytes());
        hasher.update([0]);
        hasher.update(amu_id.to_string().as_bytes());
        hasher.update([0]);
        hasher.update(content_hash.as_str().as_bytes());
        let digest: [u8; 32] = hasher.finalize().into();
        Self(ContentHash::parse(&hex::encode(digest)).expect("sha256 hex"))
    }

    pub fn parse(raw: &str) -> Result<Self> {
        Ok(Self(ContentHash::parse(raw)?))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl TryFrom<String> for KvBlockHash {
    type Error = AetherError;

    fn try_from(value: String) -> Result<Self> {
        Self::parse(&value)
    }
}

impl From<KvBlockHash> for String {
    fn from(value: KvBlockHash) -> Self {
        value.0.into()
    }
}

impl fmt::Display for KvBlockHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KvBlockMode {
    PagedPrefix,
    CacheBlendChunk,
}

impl KvBlockMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PagedPrefix => "paged_prefix",
            Self::CacheBlendChunk => "cache_blend_chunk",
        }
    }

    pub fn parse_str(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "paged_prefix" => Some(Self::PagedPrefix),
            "cache_blend_chunk" => Some(Self::CacheBlendChunk),
            _ => None,
        }
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
#[serde(rename_all = "snake_case")]
pub enum KvStorageTier {
    #[default]
    GpuHbm,
    HostDram,
    NvmeSsd,
}

impl KvStorageTier {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::GpuHbm => "gpu_hbm",
            Self::HostDram => "host_dram",
            Self::NvmeSsd => "nvme_ssd",
        }
    }

    pub fn parse_str(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "gpu_hbm" => Some(Self::GpuHbm),
            "host_dram" => Some(Self::HostDram),
            "nvme_ssd" => Some(Self::NvmeSsd),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct KvTierCapacity {
    pub gpu_hbm_max_tokens: usize,
    pub host_dram_max_tokens: usize,
    pub nvme_ssd_max_tokens: usize,
    pub bytes_per_token: usize,
}

impl Default for KvTierCapacity {
    fn default() -> Self {
        Self {
            gpu_hbm_max_tokens: 32_768,
            host_dram_max_tokens: 262_144,
            nvme_ssd_max_tokens: 2_097_152,
            bytes_per_token: 65_536,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct KvTimingProfile {
    pub cold_prefill_ms_per_1k: f64,
    pub gpu_hbm_hit_ms_per_1k: f64,
    pub host_dram_hit_ms_per_1k: f64,
    pub nvme_ssd_hit_ms_per_1k: f64,
}

impl Default for KvTimingProfile {
    fn default() -> Self {
        Self {
            cold_prefill_ms_per_1k: 120.0,
            gpu_hbm_hit_ms_per_1k: 2.0,
            host_dram_hit_ms_per_1k: 12.0,
            nvme_ssd_hit_ms_per_1k: 35.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KvCacheBlock {
    pub block_hash: KvBlockHash,
    pub model_id: ModelId,
    pub mode: KvBlockMode,
    pub tier: KvStorageTier,
    pub parent_hash: Option<KvBlockHash>,
    pub amu_id: Option<AmuId>,
    pub prefix_hash: Option<ContentHash>,
    pub scope: ScopeTag,
    pub token_count: usize,
    pub byte_size: usize,
    pub warmed_at: DateTime<Utc>,
    pub last_hit_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct KvWarmBlockInput {
    pub block: KvCacheBlock,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct KvWarmBatch {
    pub model_id: ModelId,
    pub target_tier: KvStorageTier,
    pub blocks: Vec<KvWarmBlockInput>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KvBlockHit {
    pub block_hash: KvBlockHash,
    pub tier: KvStorageTier,
    pub token_count: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KvTierUsage {
    pub gpu_hbm_blocks: usize,
    pub gpu_hbm_tokens: usize,
    pub host_dram_blocks: usize,
    pub host_dram_tokens: usize,
    pub nvme_ssd_blocks: usize,
    pub nvme_ssd_tokens: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KvAdapterWarmOutcome {
    pub blocks_warmed: usize,
    pub blocks_reused: usize,
    pub evicted_hashes: Vec<KvBlockHash>,
    pub placements: Vec<(KvBlockHash, KvStorageTier)>,
    pub fallback_used: bool,
}

pub trait KvCacheAdapter: Send + Sync {
    fn id(&self) -> &str;
    fn hydrate<'a>(&'a self, _blocks: Vec<KvCacheBlock>) -> KvFuture<'a, Result<()>> {
        Box::pin(async { Ok(()) })
    }
    fn warm_blocks<'a>(
        &'a self,
        batch: &'a KvWarmBatch,
    ) -> KvFuture<'a, Result<KvAdapterWarmOutcome>>;
    fn lookup_blocks<'a>(
        &'a self,
        model_id: &'a ModelId,
        hashes: &'a [KvBlockHash],
    ) -> KvFuture<'a, Result<Vec<KvBlockHit>>>;
    fn evict_blocks<'a>(&'a self, hashes: &'a [KvBlockHash]) -> KvFuture<'a, Result<usize>>;
    fn stats<'a>(&'a self) -> KvFuture<'a, Result<KvTierUsage>>;
}

/// Bounded in-memory KV cache enforcing HBM -> DRAM -> SSD LRU demotion.
pub struct InMemoryKvCacheAdapter {
    capacity: KvTierCapacity,
    state: Mutex<HashMap<(ModelId, KvBlockHash), KvCacheBlock>>,
}

impl InMemoryKvCacheAdapter {
    pub fn new(capacity: KvTierCapacity) -> Self {
        Self {
            capacity,
            state: Mutex::new(HashMap::new()),
        }
    }

    fn apply_warm_to_state(
        &self,
        batch: &KvWarmBatch,
        fallback_used: bool,
    ) -> Result<KvAdapterWarmOutcome> {
        let mut map = self.state.lock().map_err(|_| kv_mutex_err())?;
        let now = Utc::now();
        let mut blocks_warmed = 0usize;
        let mut blocks_reused = 0usize;

        for item in &batch.blocks {
            let key = (batch.model_id.clone(), item.block.block_hash.clone());
            if let Some(existing) = map.get_mut(&key) {
                existing.last_hit_at = now;
                if existing.tier <= batch.target_tier {
                    blocks_reused += 1;
                } else {
                    existing.tier = batch.target_tier;
                    blocks_warmed += 1;
                }
            } else {
                let mut block = item.block.clone();
                block.model_id = batch.model_id.clone();
                block.tier = batch.target_tier;
                block.last_hit_at = now;
                map.insert(key, block);
                blocks_warmed += 1;
            }
        }

        let evicted_hashes = enforce_tier_capacity(&mut map, self.capacity);
        let placements = batch
            .blocks
            .iter()
            .filter_map(|item| {
                let key = (batch.model_id.clone(), item.block.block_hash.clone());
                map.get(&key).map(|b| (b.block_hash.clone(), b.tier))
            })
            .collect();

        Ok(KvAdapterWarmOutcome {
            blocks_warmed,
            blocks_reused,
            evicted_hashes,
            placements,
            fallback_used,
        })
    }
}

impl Default for InMemoryKvCacheAdapter {
    fn default() -> Self {
        Self::new(KvTierCapacity::default())
    }
}

impl KvCacheAdapter for InMemoryKvCacheAdapter {
    fn id(&self) -> &str {
        "in-memory-tiered-kv-v1"
    }

    fn hydrate<'a>(&'a self, blocks: Vec<KvCacheBlock>) -> KvFuture<'a, Result<()>> {
        Box::pin(async move {
            let mut map = self.state.lock().map_err(|_| kv_mutex_err())?;
            for block in blocks {
                map.insert((block.model_id.clone(), block.block_hash.clone()), block);
            }
            let _ = enforce_tier_capacity(&mut map, self.capacity);
            Ok(())
        })
    }

    fn warm_blocks<'a>(
        &'a self,
        batch: &'a KvWarmBatch,
    ) -> KvFuture<'a, Result<KvAdapterWarmOutcome>> {
        Box::pin(async move { self.apply_warm_to_state(batch, false) })
    }

    fn lookup_blocks<'a>(
        &'a self,
        model_id: &'a ModelId,
        hashes: &'a [KvBlockHash],
    ) -> KvFuture<'a, Result<Vec<KvBlockHit>>> {
        Box::pin(async move {
            let mut map = self.state.lock().map_err(|_| kv_mutex_err())?;
            let now = Utc::now();
            let mut hits = Vec::new();
            for h in hashes {
                if let Some(block) = map.get_mut(&(model_id.clone(), h.clone())) {
                    block.last_hit_at = now;
                    hits.push(KvBlockHit {
                        block_hash: block.block_hash.clone(),
                        tier: block.tier,
                        token_count: block.token_count,
                    });
                }
            }
            Ok(hits)
        })
    }

    fn evict_blocks<'a>(&'a self, hashes: &'a [KvBlockHash]) -> KvFuture<'a, Result<usize>> {
        Box::pin(async move {
            if hashes.is_empty() {
                return Ok(0);
            }
            let target_set: BTreeSet<&KvBlockHash> = hashes.iter().collect();
            let mut map = self.state.lock().map_err(|_| kv_mutex_err())?;
            let before = map.len();
            map.retain(|(_, bh), _| !target_set.contains(bh));
            Ok(before - map.len())
        })
    }

    fn stats<'a>(&'a self) -> KvFuture<'a, Result<KvTierUsage>> {
        Box::pin(async move {
            let map = self.state.lock().map_err(|_| kv_mutex_err())?;
            let mut usage = KvTierUsage::default();
            for block in map.values() {
                match block.tier {
                    KvStorageTier::GpuHbm => {
                        usage.gpu_hbm_blocks += 1;
                        usage.gpu_hbm_tokens += block.token_count;
                    }
                    KvStorageTier::HostDram => {
                        usage.host_dram_blocks += 1;
                        usage.host_dram_tokens += block.token_count;
                    }
                    KvStorageTier::NvmeSsd => {
                        usage.nvme_ssd_blocks += 1;
                        usage.nvme_ssd_tokens += block.token_count;
                    }
                }
            }
            Ok(usage)
        })
    }
}

fn enforce_tier_capacity(
    map: &mut HashMap<(ModelId, KvBlockHash), KvCacheBlock>,
    capacity: KvTierCapacity,
) -> Vec<KvBlockHash> {
    demote_tier(
        map,
        KvStorageTier::GpuHbm,
        capacity.gpu_hbm_max_tokens,
        Some(KvStorageTier::HostDram),
    );
    demote_tier(
        map,
        KvStorageTier::HostDram,
        capacity.host_dram_max_tokens,
        Some(KvStorageTier::NvmeSsd),
    );
    demote_tier(
        map,
        KvStorageTier::NvmeSsd,
        capacity.nvme_ssd_max_tokens,
        None,
    )
}

fn demote_tier(
    map: &mut HashMap<(ModelId, KvBlockHash), KvCacheBlock>,
    tier: KvStorageTier,
    max_tokens: usize,
    next_tier: Option<KvStorageTier>,
) -> Vec<KvBlockHash> {
    let mut keys: Vec<((ModelId, KvBlockHash), DateTime<Utc>, usize)> = map
        .iter()
        .filter(|(_, b)| b.tier == tier)
        .map(|(k, b)| (k.clone(), b.last_hit_at, b.token_count))
        .collect();
    let mut total_tokens: usize = keys.iter().map(|(_, _, t)| *t).sum();
    if total_tokens <= max_tokens {
        return Vec::new();
    }
    keys.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));

    let mut evicted = Vec::new();
    for (key, _, tokens) in keys {
        if total_tokens <= max_tokens {
            break;
        }
        total_tokens = total_tokens.saturating_sub(tokens);
        match next_tier {
            Some(target) => {
                if let Some(block) = map.get_mut(&key) {
                    block.tier = target;
                }
            }
            None => {
                if map.remove(&key).is_some() {
                    evicted.push(key.1);
                }
            }
        }
    }
    evicted
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum InferenceEngineKind {
    #[default]
    Vllm,
    Sglang,
}

impl InferenceEngineKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Vllm => "vllm",
            Self::Sglang => "sglang",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum KvWarmProtocol {
    #[default]
    LmCacheControl,
    OpenAiPrefill,
}

#[derive(Clone)]
pub struct VllmLmCacheConfig {
    pub endpoint: String,
    pub engine: InferenceEngineKind,
    pub protocol: KvWarmProtocol,
    pub api_key: Option<String>,
    pub timeout: Duration,
    pub offline_fallback: bool,
    pub capacity: KvTierCapacity,
}

impl fmt::Debug for VllmLmCacheConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VllmLmCacheConfig")
            .field("endpoint", &self.endpoint)
            .field("engine", &self.engine)
            .field("protocol", &self.protocol)
            .field("api_key", &self.api_key.as_ref().map(|_| "[REDACTED]"))
            .field("timeout", &self.timeout)
            .field("offline_fallback", &self.offline_fallback)
            .finish()
    }
}

impl VllmLmCacheConfig {
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            engine: InferenceEngineKind::Vllm,
            protocol: KvWarmProtocol::LmCacheControl,
            api_key: None,
            timeout: Duration::from_secs(5),
            offline_fallback: true,
            capacity: KvTierCapacity::default(),
        }
    }

    pub fn with_engine(mut self, engine: InferenceEngineKind) -> Self {
        self.engine = engine;
        self
    }

    pub fn with_protocol(mut self, protocol: KvWarmProtocol) -> Self {
        self.protocol = protocol;
        self
    }

    pub fn with_api_key(mut self, api_key: impl Into<String>) -> Self {
        self.api_key = Some(api_key.into());
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn with_offline_fallback(mut self, fallback: bool) -> Self {
        self.offline_fallback = fallback;
        self
    }
}

/// HTTP/1.1 JSON client for vLLM/SGLang and LMCache with optional [`InMemoryKvCacheAdapter`] offline fallback.
pub struct VllmLmCacheHttpAdapter {
    config: VllmLmCacheConfig,
    fallback: InMemoryKvCacheAdapter,
}

impl VllmLmCacheHttpAdapter {
    pub fn new(config: VllmLmCacheConfig) -> Self {
        let fallback = InMemoryKvCacheAdapter::new(config.capacity);
        Self { config, fallback }
    }

    async fn dispatch_warm(&self, batch: &KvWarmBatch) -> Result<()> {
        let (host_port, base_path) = parse_http_endpoint(&self.config.endpoint)?;
        let (path, payload) = match self.config.protocol {
            KvWarmProtocol::LmCacheControl => {
                let blocks: Vec<serde_json::Value> = batch
                    .blocks
                    .iter()
                    .map(|item| {
                        serde_json::json!({
                            "block_hash": item.block.block_hash.as_str(),
                            "mode": item.block.mode.as_str(),
                            "parent_hash": item.block.parent_hash.as_ref().map(KvBlockHash::as_str),
                            "amu_id": item.block.amu_id.map(|id| id.to_string()),
                            "prefix_hash": item.block.prefix_hash.as_ref().map(ContentHash::as_str),
                            "scope": item.block.scope.as_str(),
                            "token_count": item.block.token_count,
                            "text": item.text,
                        })
                    })
                    .collect();
                (
                    format!("{base_path}/v1/kv/warm"),
                    serde_json::json!({
                        "engine": self.config.engine.as_str(),
                        "model": batch.model_id.as_str(),
                        "target_tier": batch.target_tier.as_str(),
                        "blocks": blocks,
                    }),
                )
            }
            KvWarmProtocol::OpenAiPrefill => {
                let prompt: String = batch.blocks.iter().map(|b| b.text.as_str()).collect();
                (
                    format!("{base_path}/v1/completions"),
                    serde_json::json!({
                        "model": batch.model_id.as_str(),
                        "prompt": prompt,
                        "max_tokens": 1,
                        "temperature": 0.0,
                        "stream": false,
                    }),
                )
            }
        };
        send_http_json_post(
            &host_port,
            &path,
            self.config.api_key.as_deref(),
            &payload,
            self.config.timeout,
        )
        .await
    }

    async fn dispatch_evict(&self, hashes: &[KvBlockHash]) -> Result<()> {
        let (host_port, base_path) = parse_http_endpoint(&self.config.endpoint)?;
        let hash_strs: Vec<&str> = hashes.iter().map(KvBlockHash::as_str).collect();
        let payload = serde_json::json!({
            "engine": self.config.engine.as_str(),
            "block_hashes": hash_strs,
        });
        send_http_json_post(
            &host_port,
            &format!("{base_path}/v1/kv/evict"),
            self.config.api_key.as_deref(),
            &payload,
            self.config.timeout,
        )
        .await
    }
}

impl KvCacheAdapter for VllmLmCacheHttpAdapter {
    fn id(&self) -> &str {
        "vllm-lmcache-http-v1"
    }

    fn hydrate<'a>(&'a self, blocks: Vec<KvCacheBlock>) -> KvFuture<'a, Result<()>> {
        self.fallback.hydrate(blocks)
    }

    fn warm_blocks<'a>(
        &'a self,
        batch: &'a KvWarmBatch,
    ) -> KvFuture<'a, Result<KvAdapterWarmOutcome>> {
        Box::pin(async move {
            if batch.blocks.is_empty() {
                return Ok(KvAdapterWarmOutcome {
                    blocks_warmed: 0,
                    blocks_reused: 0,
                    evicted_hashes: Vec::new(),
                    placements: Vec::new(),
                    fallback_used: false,
                });
            }
            match self.dispatch_warm(batch).await {
                Ok(()) => self.fallback.apply_warm_to_state(batch, false),
                Err(err) if self.config.offline_fallback => {
                    tracing::warn!(error = %err, "KV HTTP endpoint unavailable, using offline tier fallback");
                    self.fallback.apply_warm_to_state(batch, true)
                }
                Err(err) => Err(err),
            }
        })
    }

    fn lookup_blocks<'a>(
        &'a self,
        model_id: &'a ModelId,
        hashes: &'a [KvBlockHash],
    ) -> KvFuture<'a, Result<Vec<KvBlockHit>>> {
        self.fallback.lookup_blocks(model_id, hashes)
    }

    fn evict_blocks<'a>(&'a self, hashes: &'a [KvBlockHash]) -> KvFuture<'a, Result<usize>> {
        Box::pin(async move {
            if !hashes.is_empty() && self.config.protocol == KvWarmProtocol::LmCacheControl {
                if let Err(err) = self.dispatch_evict(hashes).await {
                    if !self.config.offline_fallback {
                        return Err(err);
                    }
                    tracing::warn!(error = %err, "KV HTTP evict unavailable, evicting local mirror only");
                }
            }
            self.fallback.evict_blocks(hashes).await
        })
    }

    fn stats<'a>(&'a self) -> KvFuture<'a, Result<KvTierUsage>> {
        self.fallback.stats()
    }
}

fn parse_http_endpoint(raw: &str) -> Result<(String, String)> {
    let trimmed = raw.trim().trim_end_matches('/');
    let rest = trimmed.strip_prefix("http://").ok_or_else(|| {
        AetherError::InvalidInput(format!(
            "VllmLmCacheHttpAdapter requires http:// endpoint (got `{raw}`)"
        ))
    })?;
    let (host_port_raw, base_path) = match rest.find('/') {
        Some(idx) => (&rest[..idx], rest[idx..].trim_end_matches('/').to_string()),
        None => (rest, String::new()),
    };
    if host_port_raw.is_empty() {
        return Err(AetherError::InvalidInput(format!(
            "missing host in endpoint `{raw}`"
        )));
    }
    let host_port = if host_port_raw.contains(':') {
        host_port_raw.to_string()
    } else {
        format!("{host_port_raw}:80")
    };
    Ok((host_port, base_path))
}

async fn send_http_json_post(
    host_port: &str,
    path: &str,
    api_key: Option<&str>,
    payload: &serde_json::Value,
    timeout: Duration,
) -> Result<()> {
    let body = serde_json::to_string(payload)?;
    let auth_header = match api_key.map(str::trim).filter(|k| !k.is_empty()) {
        Some(key) => format!("Authorization: Bearer {key}\r\n"),
        None => String::new(),
    };
    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: {host_port}\r\nContent-Type: application/json\r\n{auth_header}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );

    tokio::time::timeout(timeout, async {
        let mut stream =
            TcpStream::connect(host_port)
                .await
                .map_err(|err| AetherError::Transient {
                    connector: "vllm-lmcache",
                    message: format!("connect `{host_port}` failed: {err}"),
                })?;
        stream
            .write_all(req.as_bytes())
            .await
            .map_err(|err| AetherError::Transient {
                connector: "vllm-lmcache",
                message: format!("write to `{host_port}` failed: {err}"),
            })?;
        stream.flush().await.map_err(|err| AetherError::Transient {
            connector: "vllm-lmcache",
            message: format!("flush to `{host_port}` failed: {err}"),
        })?;
        let mut resp = Vec::new();
        stream
            .read_to_end(&mut resp)
            .await
            .map_err(|err| AetherError::Transient {
                connector: "vllm-lmcache",
                message: format!("read from `{host_port}` failed: {err}"),
            })?;
        let status_line = String::from_utf8_lossy(&resp)
            .lines()
            .next()
            .unwrap_or("")
            .to_string();
        let code: u16 = status_line
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        if !(200..300).contains(&code) {
            return Err(AetherError::Transient {
                connector: "vllm-lmcache",
                message: format!("unexpected HTTP status from `{host_port}`: `{status_line}`"),
            });
        }
        Ok(())
    })
    .await
    .map_err(|_| AetherError::Transient {
        connector: "vllm-lmcache",
        message: format!("KV request to `{host_port}` timed out"),
    })?
}

fn kv_mutex_err() -> AetherError {
    AetherError::Permanent {
        connector: "kv-cache",
        message: "kv cache mutex poisoned".into(),
    }
}

#[derive(Debug, Clone)]
pub struct WarmKvRequest {
    pub model_id: ModelId,
    pub scope: Option<ScopeTag>,
    pub amu_ids: Option<Vec<AmuId>>,
    pub target_tier: KvStorageTier,
    pub include_hot_prefix: bool,
    pub include_cache_blend: bool,
    pub block_tokens: usize,
}

impl WarmKvRequest {
    pub fn new(model_id: ModelId) -> Self {
        Self {
            model_id,
            scope: None,
            amu_ids: None,
            target_tier: KvStorageTier::GpuHbm,
            include_hot_prefix: true,
            include_cache_blend: true,
            block_tokens: DEFAULT_KV_BLOCK_TOKENS,
        }
    }

    pub fn with_scope(mut self, scope: ScopeTag) -> Self {
        self.scope = Some(scope);
        self
    }

    pub fn with_target_tier(mut self, tier: KvStorageTier) -> Self {
        self.target_tier = tier;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KvWarmReport {
    pub model_id: ModelId,
    pub target_tier: KvStorageTier,
    pub blocks_warmed: usize,
    pub blocks_reused: usize,
    pub tokens_warmed: usize,
    pub estimated_baseline_ttft_ms: f64,
    pub estimated_warmed_ttft_ms: f64,
    pub estimated_ttft_saved_ms: f64,
    pub ttft_reduction_pct: f64,
    pub fallback_used: bool,
    pub block_hashes: Vec<KvBlockHash>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KvRecallHint {
    pub model_id: ModelId,
    pub prefix_block_hashes: Vec<KvBlockHash>,
    pub cache_blend_hashes: Vec<KvBlockHash>,
    pub cache_hit_tokens: usize,
    pub total_tokens: usize,
    pub estimated_ttft_saved_ms: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KvCacheStats {
    pub adapter_id: String,
    pub persisted_blocks: usize,
    pub total_tokens: usize,
    pub total_bytes: usize,
    pub tier_usage: KvTierUsage,
    pub models: Vec<ModelId>,
}

pub(crate) struct KvBlockBuildSpec<'a> {
    pub(crate) model_id: &'a ModelId,
    pub(crate) scope: &'a ScopeTag,
    pub(crate) tier: KvStorageTier,
    pub(crate) block_tokens: usize,
    pub(crate) bytes_per_token: usize,
    pub(crate) now: DateTime<Utc>,
}

pub(crate) fn build_paged_prefix_blocks(
    static_prefix: &str,
    prefix_hash: &ContentHash,
    spec: &KvBlockBuildSpec<'_>,
) -> Vec<KvWarmBlockInput> {
    if static_prefix.is_empty() {
        return Vec::new();
    }
    let chunk_bytes = spec.block_tokens.max(1) * 4;
    let mut blocks = Vec::new();
    let mut parent = KvBlockHash::genesis();
    let mut start = 0usize;

    while start < static_prefix.len() {
        let mut end = (start + chunk_bytes).min(static_prefix.len());
        while end < static_prefix.len() && !static_prefix.is_char_boundary(end) {
            end += 1;
        }
        let chunk = &static_prefix[start..end];
        let hash = KvBlockHash::paged_chained(spec.model_id, &parent, chunk);
        let token_count = estimate_tokens(chunk);
        blocks.push(KvWarmBlockInput {
            block: KvCacheBlock {
                block_hash: hash.clone(),
                model_id: spec.model_id.clone(),
                mode: KvBlockMode::PagedPrefix,
                tier: spec.tier,
                parent_hash: Some(parent),
                amu_id: None,
                prefix_hash: Some(prefix_hash.clone()),
                scope: spec.scope.clone(),
                token_count,
                byte_size: token_count * spec.bytes_per_token,
                warmed_at: spec.now,
                last_hit_at: spec.now,
            },
            text: chunk.to_string(),
        });
        parent = hash;
        start = end;
    }

    blocks
}

pub(crate) fn build_cache_blend_blocks(
    amus: &[Amu],
    spec: &KvBlockBuildSpec<'_>,
) -> Vec<KvWarmBlockInput> {
    let mut blocks = Vec::new();
    for amu in amus {
        if amu.status != AmuStatus::Active || amu.tier == VaultTier::Hot {
            continue;
        }
        let line = format_recall_amu_line(amu);
        let hash = KvBlockHash::cache_blend(spec.model_id, amu.amu_id, &amu.content_hash);
        let token_count = estimate_tokens(&line);
        let scope = amu.scopes.first().cloned().unwrap_or_else(ScopeTag::global);
        blocks.push(KvWarmBlockInput {
            block: KvCacheBlock {
                block_hash: hash,
                model_id: spec.model_id.clone(),
                mode: KvBlockMode::CacheBlendChunk,
                tier: spec.tier,
                parent_hash: None,
                amu_id: Some(amu.amu_id),
                prefix_hash: None,
                scope,
                token_count,
                byte_size: token_count * spec.bytes_per_token,
                warmed_at: spec.now,
                last_hit_at: spec.now,
            },
            text: line,
        });
    }
    blocks
}

pub(crate) fn compute_ttft_savings(
    total_tokens: usize,
    hits: &[KvBlockHit],
    profile: KvTimingProfile,
) -> (f64, f64, f64, f64) {
    if total_tokens == 0 {
        return (0.0, 0.0, 0.0, 0.0);
    }
    let baseline_ms = (total_tokens as f64 / 1000.0) * profile.cold_prefill_ms_per_1k;
    let mut hbm_tok = 0usize;
    let mut dram_tok = 0usize;
    let mut ssd_tok = 0usize;

    let mut remaining = total_tokens;
    for hit in hits {
        if remaining == 0 {
            break;
        }
        let credited = hit.token_count.min(remaining);
        remaining -= credited;
        match hit.tier {
            KvStorageTier::GpuHbm => hbm_tok += credited,
            KvStorageTier::HostDram => dram_tok += credited,
            KvStorageTier::NvmeSsd => ssd_tok += credited,
        }
    }

    let warmed_ms = (remaining as f64 / 1000.0) * profile.cold_prefill_ms_per_1k
        + (hbm_tok as f64 / 1000.0) * profile.gpu_hbm_hit_ms_per_1k
        + (dram_tok as f64 / 1000.0) * profile.host_dram_hit_ms_per_1k
        + (ssd_tok as f64 / 1000.0) * profile.nvme_ssd_hit_ms_per_1k;
    let saved_ms = (baseline_ms - warmed_ms).max(0.0);
    let reduction_pct = if baseline_ms > 0.0 {
        (saved_ms / baseline_ms) * 100.0
    } else {
        0.0
    };
    (baseline_ms, warmed_ms, saved_ms, reduction_pct)
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::*;
    use crate::types::{AmuKind, AmuStatus, SourcePointer, VaultTier};

    #[tokio::test]
    async fn in_memory_kv_adapter_warms_demotes_tiers_and_reduces_ttft_over_80_pct() {
        let model = ModelId::new("Qwen/Qwen2.5-32B-Instruct").unwrap();
        let cap = KvTierCapacity {
            gpu_hbm_max_tokens: 40,
            host_dram_max_tokens: 40,
            nvme_ssd_max_tokens: 40,
            bytes_per_token: 1024,
        };
        let adapter = InMemoryKvCacheAdapter::new(cap);
        let now = Utc::now();
        let prefix =
            "Always follow strict TDD and keep commits atomic and verifiable across every module. "
                .repeat(3);
        let phash = ContentHash::compute(prefix.as_bytes());
        let scope = ScopeTag::global();
        let spec = KvBlockBuildSpec {
            model_id: &model,
            scope: &scope,
            tier: KvStorageTier::GpuHbm,
            block_tokens: 16,
            bytes_per_token: cap.bytes_per_token,
            now,
        };
        let blocks = build_paged_prefix_blocks(&prefix, &phash, &spec);
        assert!(blocks.len() >= 3);

        let batch = KvWarmBatch {
            model_id: model.clone(),
            target_tier: KvStorageTier::GpuHbm,
            blocks: blocks.clone(),
        };
        let outcome = adapter.warm_blocks(&batch).await.unwrap();
        assert_eq!(outcome.blocks_warmed, blocks.len());
        let usage = adapter.stats().await.unwrap();
        assert!(usage.gpu_hbm_tokens <= cap.gpu_hbm_max_tokens);
        assert!(usage.host_dram_tokens > 0 || usage.nvme_ssd_tokens > 0);

        let large = InMemoryKvCacheAdapter::default();
        let large_spec = KvBlockBuildSpec {
            model_id: &model,
            scope: &scope,
            tier: KvStorageTier::GpuHbm,
            block_tokens: 16,
            bytes_per_token: 65_536,
            now,
        };
        let large_blocks = build_paged_prefix_blocks(&prefix, &phash, &large_spec);
        let hashes: Vec<KvBlockHash> = large_blocks
            .iter()
            .map(|b| b.block.block_hash.clone())
            .collect();
        let total_tokens: usize = large_blocks.iter().map(|b| b.block.token_count).sum();
        large
            .warm_blocks(&KvWarmBatch {
                model_id: model.clone(),
                target_tier: KvStorageTier::GpuHbm,
                blocks: large_blocks,
            })
            .await
            .unwrap();
        let hits = large.lookup_blocks(&model, &hashes).await.unwrap();
        let (base, warmed, saved, pct) =
            compute_ttft_savings(total_tokens, &hits, KvTimingProfile::default());
        assert!(base > warmed && saved > 0.0);
        assert!(pct >= 80.0, "expected >=80% TTFT reduction, got {pct}%");
    }

    #[tokio::test]
    async fn vllm_lmcache_http_adapter_sends_requests_and_falls_back_when_offline() {
        let model = ModelId::new("meta-llama/Llama-3.3-70B-Instruct").unwrap();
        let ptr = SourcePointer::parse("Specs/KV.md#Cache").unwrap();
        let amu = Amu {
            amu_id: AmuId::from_pointer(&ptr),
            tier: VaultTier::Warm,
            kind: AmuKind::Spec,
            status: AmuStatus::Active,
            heading_path: "Specs/KV.md#Cache".into(),
            distilled_fact: "Use PagedAttention prefix sharing.".into(),
            source_pointer: ptr,
            graph_edges: Vec::new(),
            valid_from: Utc::now(),
            superseded_by: None,
            scopes: vec![ScopeTag::global()],
            authority_weight: 1.0,
            content_hash: ContentHash::compute(b"Use PagedAttention prefix sharing."),
        };
        let global = ScopeTag::global();
        let blend_spec = KvBlockBuildSpec {
            model_id: &model,
            scope: &global,
            tier: KvStorageTier::GpuHbm,
            block_tokens: DEFAULT_KV_BLOCK_TOKENS,
            bytes_per_token: 65_536,
            now: Utc::now(),
        };
        let blend = build_cache_blend_blocks(&[amu], &blend_spec);
        assert_eq!(blend.len(), 1);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server_task = tokio::spawn(async move {
            let mut seen_paths = Vec::new();
            for _ in 0..3 {
                let (mut sock, _) = listener.accept().await.unwrap();
                let mut buf = [0u8; 4096];
                let n = sock.read(&mut buf).await.unwrap();
                let req_str = String::from_utf8_lossy(&buf[..n]);
                let first_line = req_str.lines().next().unwrap_or("");
                seen_paths.push(first_line.to_string());
                let body = r#"{"status":"ok"}"#;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                sock.write_all(resp.as_bytes()).await.unwrap();
            }
            seen_paths
        });

        let lm_adapter = VllmLmCacheHttpAdapter::new(
            VllmLmCacheConfig::new(format!("http://{addr}"))
                .with_protocol(KvWarmProtocol::LmCacheControl),
        );
        let warm_res = lm_adapter
            .warm_blocks(&KvWarmBatch {
                model_id: model.clone(),
                target_tier: KvStorageTier::GpuHbm,
                blocks: blend.clone(),
            })
            .await
            .unwrap();
        assert!(!warm_res.fallback_used);
        let evict_count = lm_adapter
            .evict_blocks(&[blend[0].block.block_hash.clone()])
            .await
            .unwrap();
        assert_eq!(evict_count, 1);

        let oai_adapter = VllmLmCacheHttpAdapter::new(
            VllmLmCacheConfig::new(format!("http://{addr}"))
                .with_engine(InferenceEngineKind::Sglang)
                .with_protocol(KvWarmProtocol::OpenAiPrefill),
        );
        let oai_res = oai_adapter
            .warm_blocks(&KvWarmBatch {
                model_id: model.clone(),
                target_tier: KvStorageTier::GpuHbm,
                blocks: blend.clone(),
            })
            .await
            .unwrap();
        assert!(!oai_res.fallback_used);

        let seen = server_task.await.unwrap();
        assert!(seen[0].starts_with("POST /v1/kv/warm "));
        assert!(seen[1].starts_with("POST /v1/kv/evict "));
        assert!(seen[2].starts_with("POST /v1/completions "));

        let offline = VllmLmCacheHttpAdapter::new(
            VllmLmCacheConfig::new("http://127.0.0.1:9")
                .with_timeout(Duration::from_millis(200))
                .with_offline_fallback(true),
        );
        let fb_res = offline
            .warm_blocks(&KvWarmBatch {
                model_id: model,
                target_tier: KvStorageTier::GpuHbm,
                blocks: blend,
            })
            .await
            .unwrap();
        assert!(fb_res.fallback_used);
        assert_eq!(fb_res.blocks_warmed, 1);
    }
}
