export type VaultTier = 'hot' | 'warm' | 'cold';

export type InboxCheckbox = 'pending' | 'approved' | 'rejected';

export type InboxDecision = 'approve' | 'reject';

export interface VaultStatusReport {
  status: string;
  schema_version: number;
  embedder_id: string;
  embedding_dim: number;
  active_amus: number;
  pending_inbox_count: number;
  approved_inbox_count: number;
  hot_tokens_used: number;
  hot_token_budget: number;
  prefix_hash: string;
}

export interface InboxEntry {
  checkbox: InboxCheckbox;
  tier: VaultTier;
  scope: string;
  proposed_fact: string;
  candidate_id: string;
  target_link: string | null;
  proposed_by: string;
  proposed_date: string;
  supersedes_amu_id: string | null;
}

export interface InboxListResponse {
  inbox_path: string;
  approved_mirror_path: string;
  pending: InboxEntry[];
}

export interface InboxReconcileReport {
  compiled: number;
  rejected: number;
  remaining_pending: number;
}

export interface DecideResponse {
  candidate_id: string;
  decision: InboxDecision;
  reconcile: InboxReconcileReport;
}

export interface DreamReport {
  daily_notes_scanned: number;
  daily_notes_skipped_unchanged: number;
  candidates_staged: number;
  contradictions_flagged: number;
  staged_candidate_ids: string[];
}

export interface SyncReport {
  files_scanned: number;
  files_skipped_unchanged: number;
  files_skipped_self_write: number;
  files_updated: number;
  files_removed: number;
  private_skipped: number;
  amus_indexed: number;
  inbox_approved: number;
  inbox_rejected: number;
  reindexed_amus: number;
}

export type KvStorageTier = 'gpu_hbm' | 'host_dram' | 'nvme_ssd';

export interface GitSyncOptionsRequest {
  remote?: string;
  branch?: string;
  push?: boolean;
  commit_message?: string;
}

export interface GitSyncReport {
  remote: string;
  branch: string;
  head_commit: string;
  merged_cleanly: boolean;
  aether_conflicts_resolved: number;
  conflicted_files: string[];
  sync_report: SyncReport;
  pushed: boolean;
}

export interface WarmKvRequest {
  model_id: string;
  scope?: string;
  amu_ids?: string[];
  target_tier?: KvStorageTier;
  include_hot_prefix?: boolean;
  include_cache_blend?: boolean;
  block_tokens?: number;
}

export interface KvWarmReport {
  model_id: string;
  target_tier: KvStorageTier;
  blocks_warmed: number;
  blocks_reused: number;
  tokens_warmed: number;
  estimated_baseline_ttft_ms: number;
  estimated_warmed_ttft_ms: number;
  estimated_ttft_saved_ms: number;
  ttft_reduction_pct: number;
  fallback_used: boolean;
  block_hashes: string[];
}

export interface KvTierUsage {
  gpu_hbm_blocks: number;
  host_dram_blocks: number;
  nvme_ssd_blocks: number;
  gpu_hbm_tokens: number;
  host_dram_tokens: number;
  nvme_ssd_tokens: number;
}

export interface KvCacheStats {
  adapter_id: string;
  persisted_blocks: number;
  total_tokens: number;
  total_bytes: number;
  tier_usage: KvTierUsage;
  models: string[];
}
