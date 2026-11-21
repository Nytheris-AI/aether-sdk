---
sidebar_label: HTTP API
---

# aether-mcp HTTP API

The control plane that AMC, REST clients, and remote agents talk to.
The MCP tools are also available over HTTP at `POST /v1/mcp`, see [aether-mcp CLI and MCP tools](./aether-mcp.md).

## HTTP Control Plane & Team API (19 endpoints)

When `require_auth` is `false`, `ControlPlaneServer::bind` permits only `127.0.0.1` and `::1`. When `require_auth` is `true`, every request (except `OPTIONS` CORS preflight returning `204 No Content` and `GET /v1/openapi.json`) requires `Authorization: Bearer aek_<hex>`. Errors return `{ "error": "..." }` with status `400`, `401`, `403`, `404`, or `500`.

| Method | Path | Minimum role (when authenticated) | Request body or query | Response fields |
| --- | --- | --- | --- | --- |
| `GET` | `/v1/openapi.json` | Public (unauthenticated) | None | OpenAPI `3.1.0` specification (`bearerAuth`, `/v1/status`, `/v1/recall`, `/v1/stage`, `/v1/inspect`) |
| `GET` | `/v1/status` | Valid token (`Reader+`) | None | `VaultStatusReport` (`status`, `schema_version`, `embedder_id`, `embedding_dim`, `active_amus`, `pending_inbox_count`, `approved_inbox_count`, `hot_tokens_used`, `hot_token_budget`, `prefix_hash`) |
| `GET` | `/v1/inbox` | `Reader` | None | `inbox_path`, `approved_mirror_path`, `pending` (`Vec<InboxEntry>`) |
| `POST` | `/v1/inbox/decide` | `Admin` (plus write scope check) | `{ candidate_id, decision }` (`"approve"` or `"reject"`) | `{ candidate_id, decision, reconcile: InboxReconcileReport }` |
| `POST` | `/v1/inbox/reconcile` | `Admin` | None | `InboxReconcileReport` (`compiled`, `rejected`, `remaining_pending`) |
| `POST` | `/v1/dream` | `Admin` | None | `DreamReport` (`daily_notes_scanned`, `daily_notes_skipped_unchanged`, `candidates_staged`, `contradictions_flagged`, `staged_candidate_ids`) |
| `POST` | `/v1/sync` | `Admin` | None | `SyncReport` (`files_scanned`, `files_skipped_unchanged`, `files_skipped_self_write`, `files_updated`, `files_removed`, `private_skipped`, `amus_indexed`, `inbox_approved`, `inbox_rejected`, `reindexed_amus`) |
| `POST` | `/v1/git/sync` | `Admin` | Optional `{ remote, branch, push, commit_message }` | `GitSyncReport` (`remote`, `branch`, `head_commit`, `merged_cleanly`, `aether_conflicts_resolved`, `conflicted_files`, `sync_report`, `pushed`) |
| `GET` | `/v1/vault/pull` | `Reader` | None | `VaultPullResponse` (`manifest`, `inbox_path`, `inbox_markdown`, `approved_mirror_path`, `approved_mirror_markdown`) |
| `POST` | `/v1/vault/push` | `Contributor` (`Admin` for Inbox decisions or `run_dream`) | `{ notes?, deleted_paths?, private_paths?, inbox_markdown?, aetherignore?, run_dream? }` | `VaultPushResponse` (`sync_report`, `dream_report?`, `inbox_path`, `inbox_markdown`, `approved_mirror_path`, `approved_mirror_markdown`) |
| `POST` | `/v1/recall` | `Reader` | `{ query, scope?, max_tokens?, include_hot_profile?, kv_model? }` | `RecallResponse` (`formatted_context`, `hot_prefix`, `static_prefix`, `dynamic_context`, `hot_amus`, `warm_cold_amus`, `entries`, `static_prefix_tokens`, `dynamic_tokens`, `tokens_used`, `prefix_hash`, `kv_hint?`) |
| `POST` | `/v1/stage` | `Contributor` | `{ proposed_fact, suggested_tier?, scope?, target_note?, supersedes_amu_id?, proposed_by? }` | `{ candidate_id, inbox_path }` |
| `POST` | `/v1/inspect` | `Reader` | `{ source_pointer }` | `{ source_pointer, content }` |
| `POST` | `/v1/mcp` | Tool role (`Reader` or `Contributor`) | JSON-RPC 2.0 request object | JSON-RPC 2.0 response object |
| `POST` | `/v1/kv/warm` | `Contributor` | `{ model_id, scope?, amu_ids?, target_tier?, include_hot_prefix?, include_cache_blend?, block_tokens? }` | `KvWarmReport` (`model_id`, `target_tier`, `blocks_warmed`, `blocks_reused`, `tokens_warmed`, `estimated_baseline_ttft_ms`, `estimated_warmed_ttft_ms`, `estimated_ttft_saved_ms`, `ttft_reduction_pct`, `fallback_used`, `block_hashes`) |
| `GET` | `/v1/kv/stats` | Valid token (`Reader+`) | Optional `?model_id=<id>` | `KvCacheStats` (`adapter_id`, `persisted_blocks`, `total_tokens`, `total_bytes`, `tier_usage`, `models`) |
| `GET` | `/v1/auth/keys` | `Admin` | None | `{ keys: Vec<ScopedApiKey> }` |
| `POST` | `/v1/auth/keys` | `Admin` | `{ agent_id, role, scopes, expires_at? }` (RFC 3339) | `{ key: ScopedApiKey, secret: "aek_<64-hex>" }` |
| `POST` | `/v1/auth/keys/revoke` | `Admin` | `{ key_id }` | `{ key_id, revoked: bool }` |
