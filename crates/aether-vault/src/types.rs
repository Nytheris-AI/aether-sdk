use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use nytheris_aether_core::{AetherError, MemoryId, ProjectId, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::embedder::Embedder;
use crate::kv::{KvCacheAdapter, KvRecallHint, ModelId};
use crate::reranker::Reranker;

/// Hard token ceiling for the byte-stable static `HOT` prefix block.
pub const HOT_PROFILE_MAX_TOKENS: usize = 2000;

/// Current SQLite vault index schema version.
pub const VAULT_SCHEMA_VERSION: u32 = 3;

/// Default maximum active AMUs re-embedded synchronously inside one startup transaction.
pub const DEFAULT_SYNC_REINDEX_THRESHOLD: usize = 5_000;

/// Validated vault-relative POSIX Markdown path (`Architecture/Storage.md`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct VaultPath(String);

impl VaultPath {
    /// Parses and normalizes a vault-relative Markdown path.
    pub fn parse(raw: impl AsRef<str>) -> Result<Self> {
        let trimmed = raw.as_ref().trim();
        if trimmed.is_empty() || trimmed.contains('\0') {
            return Err(AetherError::InvalidInput(format!(
                "invalid vault-relative path: `{trimmed}`"
            )));
        }

        let replaced = trimmed.replace('\\', "/");
        let mut normalized = replaced.as_str();
        while let Some(rest) = normalized.strip_prefix("./") {
            normalized = rest;
        }

        if normalized.is_empty() || normalized.starts_with('/') || is_windows_drive_path(normalized)
        {
            return Err(AetherError::InvalidInput(format!(
                "invalid vault-relative path: `{trimmed}`"
            )));
        }

        for seg in normalized.split('/') {
            if seg.is_empty()
                || seg == "."
                || seg == ".."
                || matches!(seg, ".obsidian" | ".git" | ".trash" | ".aether")
            {
                return Err(AetherError::InvalidInput(format!(
                    "invalid or forbidden vault path segment `{seg}` in `{trimmed}`"
                )));
            }
        }

        if !normalized.to_ascii_lowercase().ends_with(".md") {
            return Err(AetherError::InvalidInput(format!(
                "vault path must point to a .md file: `{trimmed}`"
            )));
        }

        Ok(Self(normalized.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn join_on(&self, vault_root: &Path) -> PathBuf {
        self.0
            .split('/')
            .fold(vault_root.to_path_buf(), |acc, seg| acc.join(seg))
    }
}

fn is_windows_drive_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

impl TryFrom<String> for VaultPath {
    type Error = AetherError;

    fn try_from(value: String) -> Result<Self> {
        Self::parse(value)
    }
}

impl From<VaultPath> for String {
    fn from(value: VaultPath) -> Self {
        value.0
    }
}

impl fmt::Display for VaultPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Canonical provenance pointer: `path/Note.md` or `path/Note.md#Heading-Anchor`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SourcePointer {
    path: VaultPath,
    heading_anchor: Option<String>,
    canonical: String,
}

impl SourcePointer {
    /// Constructs a validated `SourcePointer` from a `VaultPath` and optional heading anchor.
    pub fn new(path: VaultPath, heading_anchor: Option<String>) -> Result<Self> {
        let cleaned_anchor = match heading_anchor {
            Some(raw) => {
                let anchor = raw.trim().to_string();
                if anchor.is_empty()
                    || anchor.contains('\n')
                    || anchor.contains('\r')
                    || anchor.contains('\0')
                {
                    return Err(AetherError::InvalidInput(
                        "heading_anchor must be non-empty single-line text".into(),
                    ));
                }
                Some(anchor)
            }
            None => None,
        };
        let canonical = match &cleaned_anchor {
            Some(anchor) => format!("{path}#{anchor}"),
            None => path.as_str().to_owned(),
        };
        Ok(Self {
            path,
            heading_anchor: cleaned_anchor,
            canonical,
        })
    }

    /// Parses and normalizes a raw provenance string (`./Arch/DB.md#Schema`).
    pub fn parse(raw: impl AsRef<str>) -> Result<Self> {
        let trimmed = raw.as_ref().trim();
        match trimmed.split_once('#') {
            Some((path_part, anchor_part)) => {
                Self::new(VaultPath::parse(path_part)?, Some(anchor_part.to_string()))
            }
            None => Self::new(VaultPath::parse(trimmed)?, None),
        }
    }

    pub fn relative_path(&self) -> &str {
        self.path.as_str()
    }

    pub fn vault_path(&self) -> &VaultPath {
        &self.path
    }

    pub fn heading_anchor(&self) -> Option<&str> {
        self.heading_anchor.as_deref()
    }

    pub fn as_str(&self) -> &str {
        &self.canonical
    }
}

impl TryFrom<String> for SourcePointer {
    type Error = AetherError;

    fn try_from(value: String) -> Result<Self> {
        Self::parse(value)
    }
}

impl From<SourcePointer> for String {
    fn from(value: SourcePointer) -> Self {
        value.canonical
    }
}

impl fmt::Display for SourcePointer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.canonical)
    }
}

/// Deterministic identifier for an Aether Memory Unit (`amu_<32-hex>`), wrapping `MemoryId`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct AmuId(MemoryId);

impl AmuId {
    /// Derives a deterministic `AmuId` from a canonical `SourcePointer` using SHA-256.
    pub fn from_pointer(pointer: &SourcePointer) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(b"aether:amu:v1:");
        hasher.update(pointer.as_str().as_bytes());
        let digest: [u8; 32] = hasher.finalize().into();
        let mut bytes = [0u8; 16];
        bytes.copy_from_slice(&digest[..16]);
        bytes[6] = (bytes[6] & 0x0f) | 0x50;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        Self(MemoryId::from_uuid(Uuid::from_bytes(bytes)))
    }

    /// Parses either `amu_<32-hex>` or a standard UUID string into an `AmuId`.
    pub fn parse(raw: &str) -> Result<Self> {
        let trimmed = raw.trim();
        let hex_part = trimmed.strip_prefix("amu_").unwrap_or(trimmed);
        let uuid = Uuid::parse_str(hex_part)
            .map_err(|err| AetherError::InvalidInput(format!("invalid amu_id `{raw}`: {err}")))?;
        Ok(Self(MemoryId::from_uuid(uuid)))
    }

    /// Returns the bijective `nytheris_aether_core::MemoryId`.
    pub fn to_memory_id(self) -> MemoryId {
        self.0
    }
}

impl TryFrom<String> for AmuId {
    type Error = AetherError;

    fn try_from(value: String) -> Result<Self> {
        Self::parse(&value)
    }
}

impl From<AmuId> for String {
    fn from(value: AmuId) -> Self {
        value.to_string()
    }
}

impl FromStr for AmuId {
    type Err = AetherError;

    fn from_str(s: &str) -> Result<Self> {
        Self::parse(s)
    }
}

impl fmt::Display for AmuId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "amu_{}", self.0 .0.simple())
    }
}

/// Deterministic Inbox candidate identifier (`cand_<12-hex>`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct CandidateId(String);

impl CandidateId {
    /// Derives a deterministic `CandidateId` from `(proposed_fact, target_note, suggested_tier)`.
    pub fn from_proposal(
        proposed_fact: &str,
        target_note: Option<&str>,
        suggested_tier: VaultTier,
    ) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(b"aether:cand:v1:");
        hasher.update(suggested_tier.as_str().as_bytes());
        hasher.update([0]);
        hasher.update(target_note.unwrap_or("").trim().as_bytes());
        hasher.update([0]);
        hasher.update(proposed_fact.trim().as_bytes());
        let digest: [u8; 32] = hasher.finalize().into();
        Self(format!("cand_{}", hex::encode(&digest[..6])))
    }

    /// Validates a `cand_<hex>` candidate identifier (`6..=32` hexadecimal characters).
    pub fn parse(raw: &str) -> Result<Self> {
        let trimmed = raw.trim().to_ascii_lowercase();
        let Some(rest) = trimmed.strip_prefix("cand_") else {
            return Err(AetherError::InvalidInput(format!(
                "invalid candidate_id `{raw}`"
            )));
        };
        if !(6..=32).contains(&rest.len()) || !rest.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(AetherError::InvalidInput(format!(
                "invalid candidate_id hex `{raw}`"
            )));
        }
        Ok(Self(trimmed))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for CandidateId {
    type Error = AetherError;

    fn try_from(value: String) -> Result<Self> {
        Self::parse(&value)
    }
}

impl From<CandidateId> for String {
    fn from(value: CandidateId) -> Self {
        value.0
    }
}

impl fmt::Display for CandidateId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Validated 64-character lowercase hex SHA-256 digest.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ContentHash(String);

impl ContentHash {
    /// Computes the SHA-256 digest of `bytes`.
    pub fn compute(bytes: &[u8]) -> Self {
        let digest = Sha256::digest(bytes);
        Self(hex::encode(digest))
    }

    /// Validates a 64-character hexadecimal SHA-256 string.
    pub fn parse(raw: &str) -> Result<Self> {
        let normalized = raw.trim().to_ascii_lowercase();
        if normalized.len() != 64 || !normalized.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(AetherError::InvalidInput(format!(
                "invalid sha256 content hash `{raw}`"
            )));
        }
        Ok(Self(normalized))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ContentHash {
    type Error = AetherError;

    fn try_from(value: String) -> Result<Self> {
        Self::parse(&value)
    }
}

impl From<ContentHash> for String {
    fn from(value: ContentHash) -> Self {
        value.0
    }
}

impl fmt::Display for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Validated lowercase scope label (such as `global` or `engineering`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ScopeTag(String);

impl ScopeTag {
    pub fn global() -> Self {
        Self("global".into())
    }

    /// Normalizes to lowercase and rejects empty or whitespace-containing scope tags.
    pub fn new(raw: impl Into<String>) -> Result<Self> {
        let raw_str = raw.into();
        let norm = raw_str.trim().to_ascii_lowercase();
        if norm.is_empty() || norm.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(AetherError::InvalidInput(format!(
                "invalid scope tag `{raw_str}`"
            )));
        }
        Ok(Self(norm))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for ScopeTag {
    fn default() -> Self {
        Self::global()
    }
}

impl TryFrom<String> for ScopeTag {
    type Error = AetherError;

    fn try_from(value: String) -> Result<Self> {
        Self::new(value)
    }
}

impl From<ScopeTag> for String {
    fn from(value: ScopeTag) -> Self {
        value.0
    }
}

impl fmt::Display for ScopeTag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Non-zero strict token ceiling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TokenBudget(usize);

impl TokenBudget {
    pub fn new(tokens: usize) -> Result<Self> {
        if tokens == 0 {
            return Err(AetherError::InvalidInput(
                "token budget must be greater than zero".into(),
            ));
        }
        Ok(Self(tokens))
    }

    pub fn get(self) -> usize {
        self.0
    }
}

impl Default for TokenBudget {
    fn default() -> Self {
        Self(1500)
    }
}

/// Non-private vault memory tier. `private` is excluded at parse boundaries and cannot be constructed here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum VaultTier {
    Hot,
    #[default]
    Warm,
    Cold,
}

impl VaultTier {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hot => "hot",
            Self::Warm => "warm",
            Self::Cold => "cold",
        }
    }

    pub fn parse_str(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "hot" => Some(Self::Hot),
            "warm" => Some(Self::Warm),
            "cold" => Some(Self::Cold),
            _ => None,
        }
    }
}

impl fmt::Display for VaultTier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Semantic kind of an Obsidian note or compiled AMU.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AmuKind {
    Profile,
    Rule,
    Decision,
    #[default]
    Spec,
    Fleeting,
}

impl AmuKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Profile => "profile",
            Self::Rule => "rule",
            Self::Decision => "decision",
            Self::Spec => "spec",
            Self::Fleeting => "fleeting",
        }
    }

    pub fn parse_str(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "profile" => Some(Self::Profile),
            "rule" => Some(Self::Rule),
            "decision" => Some(Self::Decision),
            "spec" => Some(Self::Spec),
            "fleeting" => Some(Self::Fleeting),
            _ => None,
        }
    }
}

impl fmt::Display for AmuKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Lifecycle status of an AMU.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AmuStatus {
    #[default]
    Active,
    Superseded,
    Archived,
}

impl AmuStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Superseded => "superseded",
            Self::Archived => "archived",
        }
    }

    pub fn parse_str(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "active" => Some(Self::Active),
            "superseded" => Some(Self::Superseded),
            "archived" => Some(Self::Archived),
            _ => None,
        }
    }
}

impl fmt::Display for AmuStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Distilled, temporally tracked Aether Memory Unit derived from an Obsidian section or Inbox approval.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Amu {
    pub amu_id: AmuId,
    pub tier: VaultTier,
    pub kind: AmuKind,
    pub status: AmuStatus,
    pub heading_path: String,
    pub distilled_fact: String,
    pub source_pointer: SourcePointer,
    pub graph_edges: Vec<String>,
    pub valid_from: DateTime<Utc>,
    pub superseded_by: Option<AmuId>,
    pub scopes: Vec<ScopeTag>,
    pub authority_weight: f32,
    pub content_hash: ContentHash,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DbTarget {
    File(PathBuf),
    InMemory,
}

#[derive(Clone)]
pub struct VaultConfig {
    pub vault_root: PathBuf,
    pub db_target: DbTarget,
    pub project_id: ProjectId,
    pub watch_interval: Option<Duration>,
    pub hot_budget: TokenBudget,
    pub sync_reindex_threshold: usize,
    pub(crate) embedder_override: Option<Arc<dyn Embedder>>,
    pub(crate) reranker_override: Option<Arc<dyn Reranker>>,
    pub(crate) kv_adapter_override: Option<Arc<dyn KvCacheAdapter>>,
    pub default_kv_model: Option<ModelId>,
}

impl fmt::Debug for VaultConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VaultConfig")
            .field("vault_root", &self.vault_root)
            .field("db_target", &self.db_target)
            .field("project_id", &self.project_id)
            .field("watch_interval", &self.watch_interval)
            .field("hot_budget", &self.hot_budget)
            .field("sync_reindex_threshold", &self.sync_reindex_threshold)
            .field(
                "embedder_id",
                &self.embedder_override.as_ref().map(|e| e.id()),
            )
            .field(
                "reranker_id",
                &self.reranker_override.as_ref().map(|r| r.id()),
            )
            .field(
                "kv_adapter_id",
                &self.kv_adapter_override.as_ref().map(|k| k.id()),
            )
            .field("default_kv_model", &self.default_kv_model)
            .finish()
    }
}

impl VaultConfig {
    pub fn new(vault_root: impl Into<PathBuf>) -> Self {
        let root = vault_root.into();
        let db = root.join(".aether").join("aether.sqlite");
        Self {
            vault_root: root,
            db_target: DbTarget::File(db),
            project_id: ProjectId::new("obsidian-vault").expect("static project id is valid"),
            watch_interval: None,
            hot_budget: TokenBudget(HOT_PROFILE_MAX_TOKENS),
            sync_reindex_threshold: DEFAULT_SYNC_REINDEX_THRESHOLD,
            embedder_override: None,
            reranker_override: None,
            kv_adapter_override: None,
            default_kv_model: None,
        }
    }

    pub fn in_memory(vault_root: impl Into<PathBuf>) -> Self {
        Self {
            db_target: DbTarget::InMemory,
            ..Self::new(vault_root)
        }
    }

    pub fn with_db_path(mut self, db_path: impl Into<PathBuf>) -> Self {
        self.db_target = DbTarget::File(db_path.into());
        self
    }

    pub fn with_watch_interval(mut self, interval: Option<Duration>) -> Self {
        self.watch_interval = interval;
        self
    }

    pub fn with_hot_budget(mut self, budget: TokenBudget) -> Self {
        self.hot_budget = budget;
        self
    }

    pub fn with_sync_reindex_threshold(mut self, threshold: usize) -> Self {
        self.sync_reindex_threshold = threshold;
        self
    }

    pub fn with_embedder(mut self, embedder: Arc<dyn Embedder>) -> Self {
        self.embedder_override = Some(embedder);
        self
    }

    pub fn with_reranker(mut self, reranker: Arc<dyn Reranker>) -> Self {
        self.reranker_override = Some(reranker);
        self
    }

    pub fn with_kv_adapter(mut self, adapter: Arc<dyn KvCacheAdapter>) -> Self {
        self.kv_adapter_override = Some(adapter);
        self
    }

    pub fn with_default_kv_model(mut self, model: ModelId) -> Self {
        self.default_kv_model = Some(model);
        self
    }
}

#[derive(Debug, Clone)]
pub struct RecallRequest {
    pub query: String,
    pub scope: Option<ScopeTag>,
    pub max_tokens: TokenBudget,
    pub include_hot_profile: bool,
    pub kv_model: Option<ModelId>,
}

impl RecallRequest {
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            scope: None,
            max_tokens: TokenBudget::default(),
            include_hot_profile: true,
            kv_model: None,
        }
    }

    pub fn with_kv_model(mut self, model: ModelId) -> Self {
        self.kv_model = Some(model);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecallEntry {
    pub amu: Amu,
    pub score: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecallResponse {
    pub formatted_context: String,
    pub hot_prefix: String,
    pub static_prefix: String,
    pub dynamic_context: String,
    pub hot_amus: Vec<Amu>,
    pub warm_cold_amus: Vec<Amu>,
    pub entries: Vec<RecallEntry>,
    pub static_prefix_tokens: usize,
    pub dynamic_tokens: usize,
    pub tokens_used: usize,
    pub prefix_hash: ContentHash,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kv_hint: Option<KvRecallHint>,
}

#[derive(Debug, Clone)]
pub struct StageMemoryRequest {
    pub proposed_fact: String,
    pub suggested_tier: VaultTier,
    pub target_note: Option<String>,
    pub supersedes_amu_id: Option<AmuId>,
    pub scope: ScopeTag,
    pub proposed_by: String,
    pub proposed_date: String,
}

impl StageMemoryRequest {
    pub fn new(proposed_fact: impl Into<String>, suggested_tier: VaultTier) -> Self {
        Self {
            proposed_fact: proposed_fact.into(),
            suggested_tier,
            target_note: None,
            supersedes_amu_id: None,
            scope: ScopeTag::global(),
            proposed_by: "mcp-agent".into(),
            proposed_date: Utc::now().format("%Y-%m-%d").to_string(),
        }
    }

    pub fn with_scope(mut self, raw: impl Into<String>) -> Result<Self> {
        self.scope = ScopeTag::new(raw)?;
        Ok(self)
    }

    pub fn with_target_note(mut self, target_note: impl Into<String>) -> Self {
        self.target_note = Some(target_note.into());
        self
    }

    pub fn with_supersedes(mut self, amu_id: AmuId) -> Self {
        self.supersedes_amu_id = Some(amu_id);
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboxCheckbox {
    Pending,
    Approved,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxEntry {
    pub checkbox: InboxCheckbox,
    pub tier: VaultTier,
    pub scope: ScopeTag,
    pub proposed_fact: String,
    pub candidate_id: CandidateId,
    pub target_link: Option<String>,
    pub proposed_by: String,
    pub proposed_date: String,
    pub supersedes_amu_id: Option<AmuId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultMeta {
    pub schema_version: u32,
    pub embedder_id: String,
    pub embedding_dim: usize,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovedMirrorEntry {
    pub candidate_id: CandidateId,
    pub compiled_amu_id: AmuId,
    pub tier: VaultTier,
    pub scope: ScopeTag,
    pub proposed_fact: String,
    pub target_link: Option<String>,
    pub proposed_by: String,
    pub proposed_date: String,
    pub supersedes_amu_id: Option<AmuId>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncReport {
    pub files_scanned: usize,
    pub files_skipped_unchanged: usize,
    pub files_skipped_self_write: usize,
    pub files_updated: usize,
    pub files_removed: usize,
    pub private_skipped: usize,
    pub amus_indexed: usize,
    pub inbox_approved: usize,
    pub inbox_rejected: usize,
    pub reindexed_amus: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxReconcileReport {
    pub compiled: usize,
    pub rejected: usize,
    pub remaining_pending: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DreamReport {
    pub daily_notes_scanned: usize,
    pub daily_notes_skipped_unchanged: usize,
    pub candidates_staged: usize,
    pub contradictions_flagged: usize,
    pub staged_candidate_ids: Vec<CandidateId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultStatusReport {
    pub status: String,
    pub schema_version: u32,
    pub embedder_id: String,
    pub embedding_dim: usize,
    pub active_amus: usize,
    pub pending_inbox_count: usize,
    pub approved_inbox_count: usize,
    pub hot_tokens_used: usize,
    pub hot_token_budget: usize,
    pub prefix_hash: ContentHash,
}

/// Deterministic token estimator used by prefix budgeting (`ceil(len / 4)`, `0` when empty).
pub fn estimate_tokens(text: &str) -> usize {
    if text.is_empty() {
        0
    } else {
        text.len().div_ceil(4).max(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_pointer_normalizes_and_rejects_traversal() {
        let parsed = SourcePointer::parse(r".\Arch\DB.md#Token Rotation").unwrap();
        assert_eq!(parsed.relative_path(), "Arch/DB.md");
        assert_eq!(parsed.heading_anchor(), Some("Token Rotation"));
        assert_eq!(parsed.as_str(), "Arch/DB.md#Token Rotation");

        assert!(VaultPath::parse("../../etc/passwd").is_err());
        assert!(VaultPath::parse("/abs/note.md").is_err());
        assert!(VaultPath::parse(r"C:\abs\note.md").is_err());
        assert!(VaultPath::parse(".obsidian/workspace.md").is_err());
        assert!(VaultPath::parse(".git/config.md").is_err());
        assert!(VaultPath::parse(".trash/old.md").is_err());
        assert!(VaultPath::parse("notes/image.png").is_err());
        assert!(SourcePointer::parse("Arch/DB.md#").is_err());
    }

    #[test]
    fn amu_id_maps_deterministically_to_memory_id() {
        let p1 = SourcePointer::parse("Specs/Auth.md#Token-Rotation").unwrap();
        let p2 = SourcePointer::parse("./Specs/Auth.md#Token-Rotation").unwrap();
        let p3 = SourcePointer::parse("Specs/Auth.md#Other").unwrap();

        let id1 = AmuId::from_pointer(&p1);
        let id2 = AmuId::from_pointer(&p2);
        let id3 = AmuId::from_pointer(&p3);

        assert_eq!(id1, id2);
        assert_eq!(id1.to_memory_id(), id2.to_memory_id());
        assert_ne!(id1, id3);

        let parsed = AmuId::parse(&id1.to_string()).unwrap();
        assert_eq!(parsed, id1);
        assert!(AmuId::parse("not-an-id").is_err());
    }

    #[test]
    fn candidate_id_content_hash_and_scope_validate_boundaries() {
        let c1 = CandidateId::from_proposal(
            "Switched primary local vector store to LanceDB.",
            Some("Architecture/Storage-Engine"),
            VaultTier::Warm,
        );
        let c2 = CandidateId::from_proposal(
            "Switched primary local vector store to LanceDB.",
            Some("Architecture/Storage-Engine"),
            VaultTier::Warm,
        );
        assert_eq!(c1, c2);
        assert!(c1.as_str().starts_with("cand_"));
        assert_eq!(c1.as_str().len(), 17);
        assert_eq!(CandidateId::parse(c1.as_str()).unwrap(), c1);
        assert!(CandidateId::parse("cand_9f82a1").is_ok());
        assert!(CandidateId::parse("cand_ab").is_err());
        assert!(CandidateId::parse("cand_xyz").is_err());
        assert!(TokenBudget::new(0).is_err());

        let hash = ContentHash::compute(b"aether");
        assert_eq!(hash.as_str().len(), 64);
        assert_eq!(ContentHash::parse(hash.as_str()).unwrap(), hash);
        assert!(ContentHash::parse("short").is_err());

        let scope = ScopeTag::new("  Engineering ").unwrap();
        assert_eq!(scope.as_str(), "engineering");
        assert!(ScopeTag::new("   ").is_err());
        assert!(ScopeTag::new("bad scope").is_err());

        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("a"), 1);
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcde"), 2);
    }
}
