//! Obsidian-native memory compiler, hybrid recall engine, scoped team Git/HTTP gateway, KV-cache warmer, and MCP server for Project Aether.

mod auth;
mod dream;
mod embedder;
mod engine;
mod git;
mod http;
mod inbox;
mod kv;
mod mcp;
mod parser;
mod prefix;
mod reranker;
mod store;
mod types;

pub use auth::{
    AgentId, AgentRole, ApiKeySecret, AuthError, IssueApiKeyRequest, IssuedApiKey, KeyId,
    ScopedApiKey,
};
pub use embedder::{
    ComputeDevice, DenseSubwordEmbedder, Embedder, GpuEmbedderConfig, HashedNgramEmbedder,
    LocalGpuEmbedder, DEFAULT_DENSE_EMBED_DIM, DEFAULT_EMBED_DIM,
};
pub use engine::VaultEngine;
pub use git::{GitSyncOptions, GitSyncReport};
pub use http::ControlPlaneServer;
pub use kv::{
    InMemoryKvCacheAdapter, InferenceEngineKind, KvAdapterWarmOutcome, KvBlockHash, KvBlockHit,
    KvBlockMode, KvCacheAdapter, KvCacheBlock, KvCacheStats, KvFuture, KvRecallHint, KvStorageTier,
    KvTierCapacity, KvTierUsage, KvTimingProfile, KvWarmBatch, KvWarmBlockInput, KvWarmProtocol,
    KvWarmReport, ModelId, VllmLmCacheConfig, VllmLmCacheHttpAdapter, WarmKvRequest,
    DEFAULT_KV_BLOCK_TOKENS,
};
pub use mcp::McpServer;
pub use reranker::{CrossEncoderReranker, NoopReranker, Reranker, DEFAULT_RERANK_TOP_K};
pub use types::{
    estimate_tokens, Amu, AmuId, AmuKind, AmuStatus, ApprovedMirrorEntry, CandidateId, ContentHash,
    DbTarget, DreamReport, InboxCheckbox, InboxEntry, InboxReconcileReport, RecallEntry,
    RecallRequest, RecallResponse, ScopeTag, SourcePointer, StageMemoryRequest, SyncReport,
    TokenBudget, VaultConfig, VaultMeta, VaultPath, VaultStatusReport, VaultTier,
    DEFAULT_SYNC_REINDEX_THRESHOLD, HOT_PROFILE_MAX_TOKENS, VAULT_SCHEMA_VERSION,
};
