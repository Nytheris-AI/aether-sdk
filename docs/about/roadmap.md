# Aether SDK Roadmap

Aether is a model-free Rust **memory SDK**: composable pieces developers call from their own workflow to store, index, and retrieve AI memory across **SQLite, Redis/Valkey, Turbopuffer, Qdrant, Chroma, pgvector, and Neo4j**.
Architecture decisions live in the [pieces-first SDK design](../architecture/sdk-design.md) and [Phase 5 hardening design](../architecture/hardening-design.md).

## Current Status

| Area | Status | Notes |
|------|--------|-------|
| `nytheris-aether-core` contracts | Done | Newtypes (`ProjectId`, `MemoryId`, `EntityId`, `UserId`, `SessionId`), `RecordStore` / `VectorStore` / `GraphStore`, `IndexSpec`, shared scoring & filters, `AetherError`. |
| `nytheris-aether-adapters::in_memory` + `contract-suite` | Done | Thread-safe `InMemoryBackend` with `FaultPoint` injection + reusable `contract::{record_store, record_store_concurrent_idempotency, vector_store, graph_store}`. |
| Middleware (`Batcher`, `Timeout`, `Resilient`, `Traced`) | Done | `Batcher` -> `Timeout` -> `Resilient` (exponential backoff + full jitter via `jittered`) -> `Traced`. Applied by default in `MemoryManager::builder`. |
| `nytheris-aether-cortex` (`Indexer`, `recall`, `ppr`) | Done | Durable `RecordStore`-driven sync engine (`run_pass`, `apply`, opt-in `spawn`), composable recall steps, `PersonalizedPageRank` + `graph_scores`, `RRF` fusion. |
| `MemoryManager` facade (`nytheris-aether-sdk`) | Done | Optional builder accepting `RecordStore` + optional `.vector(...)`, `.graph(...)`, `.background_indexing(...)`, middleware toggles. |
| Production drivers (`sqlite`, `redis`, `turbopuffer`, `qdrant`, `chroma`, `pgvector`, `neo4j`) | Done | Implemented behind feature flags and verified via contract unit tests + live local Docker verification (`Valkey`, `Qdrant`, `Chroma`, `pgvector`, `Neo4j`) + `SQLite`. |
| Obsidian vault & MCP server (`nytheris-aether-vault`, `aether-mcp`, `obsidian-aether-plugin`) | Done | `VaultEngine` over SQLite WAL (schema `v3`) + FTS5 + `PersonalizedPageRank` + `CrossEncoderReranker`, `vault_meta` auto-reindexing, `Aether/Compiled/Approved.md` mirror, Daily Note Dream consolidation, per-agent scoped API keys, Git team sync with monotonic lattice conflict resolution, vLLM/SGLang + LMCache KV-tensor warming, authenticated `ControlPlaneServer`, scoped 3-tool `McpServer`, and Obsidian companion plugin. |
| Observability & Rust CI | Done | End-to-end `tracing` spans + `otel` OTLP preset; GitHub Actions checks `cargo fmt`, `clippy -D warnings`, `cargo test --workspace --all-features`, and docs build. |

## Completed Milestones

### Phase 5 - Pieces-First SDK & Consistency Hardening (Completed 2026-11-18)

- Replaced in-process `IngestionBuffer`, `HotContextCache`, `PolicyRouter`, and `QueryPlanner` with `RecordStore` versioned durability (`create`, `get_many`, `cas_update`, `tombstone`, `pending`, `pending_for`, `session_items`, `mark_indexed`) and `nytheris_aether_cortex::{Indexer, recall}`.
- Removed `MemoryInbound` in favor of inherent methods on `MemoryManager` assembled solely from public pieces.
- Enforced `VectorStore::describe` / `IndexSpec` validation at ingest/update/query boundaries and required matching embeddings when updating content.
- Added identity-carrying `MemoryScope::{Project, User(UserId), Session(SessionId)}` with record-only TTL-bound session storage.
- Wired `PersonalizedPageRank` + `graph_scores` into `RecallMode::Hybrid` via Reciprocal Rank Fusion (`recall::rrf`) and added `jittered` exponential backoff + `Timeout` + `Resilient` defaults on `MemoryManager::builder`.

### Phase 6 - Production Backends & Live Verification (Completed 2026-11-18)

- Implemented and upgraded all 7 backend drivers (`sqlite`, `redis`, `turbopuffer`, `qdrant`, `chroma`, `pgvector`, `neo4j`) to Phase 5 contracts including persistent cross-restart `describe` resolution on `qdrant`, `chroma`, and `pgvector`.
- Verified `sqlite` + mocked suites (`turbopuffer`, `qdrant`, `chroma`, `neo4j`, `pgvector`) offline in CI, plus live Docker integration and end-to-end `MemoryManager` suites against Valkey, Qdrant, Chroma, pgvector, and Neo4j.

### Milestone 1 - Obsidian Vault Daemon & Stdio MCP Server (Completed 2026-11-18)

- Added the fifth workspace crate `nytheris-aether-vault` (`crates/aether-vault`) and the `aether-mcp` stdio binary (`--vault`, `--db`, `--watch-ms`, `AETHER_VAULT_PATH`), documented in [M1 vault and MCP architecture](../architecture/m1-vault-mcp-rationale.md).
- Implemented validated vault types (`VaultPath`, `SourcePointer`, `AmuId`, `CandidateId`, `ContentHash`, `ScopeTag`, `TokenBudget`, `VaultTier`, `AmuKind`, `AmuStatus`), `.aetherignore` plus built-in path exclusions, and parse-boundary rejection of `aether_tier: private` notes.
- Implemented single-file SQLite WAL storage (`vault_files`, `amus`, `amus_fts`, `amu_edges`, `self_write_ledger`, `staged_candidates`), hybrid FTS5 + 256-dim `HashedNgramEmbedder` + `PersonalizedPageRank` RRF recall, deterministic `[AETHER_HOT_PROFILE]` prefix packing (`HOT_PROFILE_MAX_TOKENS = 2000`), and atomic `Aether/Inbox.md` staging and reconciliation.
- Exposed `McpServer` over newline-delimited JSON-RPC 2.0 (`initialize`, `ping`, `tools/list`, `tools/call`) with `aether_recall`, `aether_stage_memory`, and `aether_inspect_source`.

### Milestone 2 - Compiled Mirror, GPU/Dense Reranking, Dream Worker & Obsidian Companion (Completed 2026-11-18)

- Added `Aether/Compiled/Approved.md` read-only compiled mirror for `[x]` approved Inbox candidates with `aether_inspect_source` provenance support and `vault_meta` dimension-aware auto-reindexing (synchronous up to `DEFAULT_SYNC_REINDEX_THRESHOLD = 5000` AMUs, asynchronous `amus_shadow_embeddings` swap above threshold).
- Exposed public `Embedder` (`HashedNgramEmbedder`, `DenseSubwordEmbedder`, `LocalGpuEmbedder` with automatic fallback) and `Reranker` (`CrossEncoderReranker` top-`30` RRF reranking, `NoopReranker`) traits configurable on `VaultConfig`.
- Extended `RecallResponse` and `aether_recall` with `static_prefix` / `dynamic_context` separation (`structured: true` and `structuredContent`).
- Added incremental Daily Note Dream consolidation (`VaultEngine::run_dream_pass`, `dream_watermarks`, and temporal contradiction detection into `supersedes_amu_id`).
- Added loopback-only `ControlPlaneServer` (`GET /v1/status`, `GET /v1/inbox`, `POST /v1/inbox/decide`, `POST /v1/inbox/reconcile`, `POST /v1/dream`, `POST /v1/sync`, `--http-addr` / `AETHER_HTTP_ADDR`) and the TypeScript `obsidian-aether-plugin` companion plugin.

### Milestone 3 - Team Git Vaults, Scoped API Keys & Tier 2 KV Serving (Completed 2026-11-18)

- Added per-agent scoped API keys (`AgentId`, `KeyId`, `AgentRole::{Reader, Contributor, Admin}`, `ApiKeySecret` with `aek_<hex>` format and constant-time SHA-256 verification inside `agent_api_keys`) plus `VaultEngine::{issue_api_key, ensure_bootstrap_admin_key, authenticate_api_key, list_api_keys, revoke_api_key, recall_as, stage_memory_as, inspect_source_as, pending_inbox_as, warm_kv_cache_as}`.
- Added Git-backed team vault sync (`VaultEngine::git_sync`, `GitSyncOptions`, `GitSyncReport` with credential redaction via `redact_url_userinfo`), `.gitignore` `.aether/` protection, fenced-code-aware conflict marker detection skipping unresolved human notes while preserving existing SQLite AMUs, and monotonic lattice reconciliation (`Approved > Rejected > Pending` by `CandidateId`) for `Aether/Inbox.md` and `Aether/Compiled/Approved.md`.
- Added vLLM/SGLang and LMCache KV-tensor warming (`InMemoryKvCacheAdapter`, `VllmLmCacheHttpAdapter` supporting `LmCacheControl` and `OpenAiPrefill` with offline fallback) partitioning `static_prefix` into PagedAttention chained prefix blocks and `WARM`/`COLD` AMUs into CacheBlend chunks across `GpuHbm`, `HostDram`, and `NvmeSsd`, persisted in SQLite `kv_cache_blocks` with atomic eviction on AMU updates, deletions, or superseding approvals.
- Extended `ControlPlaneServer` (`ControlPlaneServer::bind_with_auth` with `/v1/recall`, `/v1/stage`, `/v1/inspect`, `/v1/mcp`, `/v1/git/sync`, `/v1/kv/warm`, `/v1/kv/stats`, `/v1/auth/keys`, `/v1/auth/keys/revoke`), scoped `McpServer` (`McpServer::scoped`, `McpServer::with_api_key`, `kv_model` -> `kv_hint`, default `scope` = `agent.default_write_scope()`), `aether-mcp` flags (`--require-auth`, `--bootstrap-admin-key`, `--api-key`, `--kv-endpoint`, `--kv-model`), and `obsidian-aether-plugin` (`AetherClientOptions`, remote HTTPS + bearer auth, `aether-git-sync`, `aether-warm-kv-cache`).

## Scope & Next Directions

- **Language Bindings (deferred)**: Foreign-language bindings (for example Python via `PyO3`) remain deferred until demanded by concrete downstream users.
