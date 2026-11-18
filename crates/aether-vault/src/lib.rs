//! Obsidian-native memory compiler, hybrid recall engine, and stdio MCP server for Project Aether.

mod embedder;
mod engine;
mod inbox;
mod mcp;
mod parser;
mod prefix;
mod reranker;
mod store;
mod types;

pub use embedder::{
    ComputeDevice, DenseSubwordEmbedder, Embedder, GpuEmbedderConfig, HashedNgramEmbedder,
    LocalGpuEmbedder, DEFAULT_DENSE_EMBED_DIM, DEFAULT_EMBED_DIM,
};
pub use engine::VaultEngine;
pub use mcp::McpServer;
pub use reranker::{CrossEncoderReranker, NoopReranker, Reranker, DEFAULT_RERANK_TOP_K};
pub use types::{
    estimate_tokens, Amu, AmuId, AmuKind, AmuStatus, ApprovedMirrorEntry, CandidateId, ContentHash,
    DbTarget, InboxCheckbox, InboxEntry, InboxReconcileReport, RecallEntry, RecallRequest,
    RecallResponse, ScopeTag, SourcePointer, StageMemoryRequest, SyncReport, TokenBudget,
    VaultConfig, VaultMeta, VaultPath, VaultTier, DEFAULT_SYNC_REINDEX_THRESHOLD,
    HOT_PROFILE_MAX_TOKENS, VAULT_SCHEMA_VERSION,
};
