//! Obsidian-native memory compiler, hybrid recall engine, and stdio MCP server for Project Aether.

#[allow(dead_code)]
mod parser;
mod types;

pub use types::{
    estimate_tokens, Amu, AmuId, AmuKind, AmuStatus, CandidateId, ContentHash, DbTarget,
    InboxCheckbox, InboxEntry, InboxReconcileReport, RecallEntry, RecallRequest, RecallResponse,
    ScopeTag, SourcePointer, StageMemoryRequest, SyncReport, TokenBudget, VaultConfig, VaultPath,
    VaultTier, HOT_PROFILE_MAX_TOKENS,
};
