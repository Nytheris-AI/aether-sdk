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
