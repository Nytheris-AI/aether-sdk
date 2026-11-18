# Memory Model & Consistency

Aether treats the **`RecordStore`** as the durable source of truth and keeps **`VectorStore`** and **`GraphStore`** as derived indexes synchronized via versioned indexing state and tombstones.

## Storage Roles

- **`RecordStore`**: Holds canonical `MemoryItem` records, atomic idempotency keys (`create`), optimistic CAS versions (`cas_update`), pending indexing state (`pending`, `pending_for`, `mark_indexed`), versioned `tombstone` deletes, and TTL-bound `session_items`.
- **`VectorStore`** *(optional)*: Holds dense vectors paired with filterable metadata (`_aether_scope`, `_aether_tier`, and caller JSON fields). Recall resolves hit bodies from `RecordStore::get_many`, omitting stale or tombstoned records.
- **`GraphStore`** *(optional)*: Holds entities and idempotent temporal edges (`source_memory_id`, `valid_at`, `invalid_at`) retracted on `update` and `forget`.

## Memory Tiers (`MemoryTier`)

`MemoryTier` (`Episodic`, `Semantic` *(default)*, `Procedural`, `Profile`) is stored on every `MemoryItem` and written to vector metadata as `_aether_tier` for filtering in queries.

## Memory Scopes (`MemoryScope`)

- **`Project`** *(default)*: Shared within a `ProjectId` namespace.
- **`User(UserId)`**: Isolated to a validated user identity via `_aether_scope` index filtering and `recall::in_scopes`.
- **`Session(SessionId)`**: Stored **only** in the `RecordStore` with TTL expiration (`with_session_ttl`, default 1 hour). Session items are never written to vector or graph indexes, preventing stale turns from outliving TTL.

## Durability & Consistency

### `WriteDurability`

- **`Buffered`** *(default)*: Persists the record in `RecordStore`, wakes any opt-in background indexer, and returns `ReceiptState::Buffered` immediately. Pending records survive process restarts and are indexed on `flush()` or by `background_indexing`.
- **`WaitFlush`**: Persists the record and runs `Indexer::apply` before returning `ReceiptState::Persisted`.

### `ConsistencyMode`

- **`ReadYourWrites`** *(default)*: Combines indexed `vector_search` hits with unindexed pending records scored using the index's `DistanceMetric::score`, keeping the newest `version` via `recall::merge`, plus live `session_items` when requested.
- **`Eventual`**: Reads derived vector/graph indexes directly (plus explicit `Session` scopes) without scanning unindexed pending records.

### Updates, Tombstones, and Graph Fusion

- Updating `content` on an embedded memory requires a matching new `embedding` so indexed vectors stay aligned with text.
- `forget` records an immediate `tombstone` (hiding the memory from `get`/`get_many` right away) and retracts vector/graph entries; any transient index delete failure stays pending and retries cleanly on subsequent flushes.
- `RecallMode::Hybrid` fuses vector and graph rankings deterministically using `recall::rrf` (Reciprocal Rank Fusion) backed by `PersonalizedPageRank` + `graph_scores`.

---

## Vault Memory Model (`nytheris-aether-vault`)

`nytheris-aether-vault` compiles Obsidian Markdown notes into tracked Aether Memory Units (`Amu`) inside a single SQLite WAL file while reading live source sections directly from disk.

### Vault Tiers (`VaultTier`) & Privacy Boundary

- **`Hot`** (`aether_tier: hot`): Packed into the static `[AETHER_HOT_PROFILE]` prefix block (`static_prefix` / `hot_prefix`) sorted deterministically by `amu_id` ascending within `HOT_PROFILE_MAX_TOKENS` (`2000`), with no volatile timestamps so `prefix_hash` stays stable across turns.
- **`Warm`** *(default)* (`aether_tier: warm`) and **`Cold`** (`aether_tier: cold`): Ranked dynamically per query by fusing SQLite FTS5 BM25, cosine vector similarity (`HashedNgramEmbedder`, `DenseSubwordEmbedder`, or `LocalGpuEmbedder`), and wikilink/tag `PersonalizedPageRank` (`graph_scores`) via `recall::rrf`, scaled by `authority_weight`, reranked over the top `30` RRF entries by `CrossEncoderReranker`, and packed into `[AETHER_RECALL_CONTEXT]` (`dynamic_context`) within `max_tokens` (default `1500`).
- **`private`** (`aether_tier: private`): Exists only during parsing (`RawFrontmatterTier`) and cannot be constructed in `VaultTier`. Private notes are removed from SQLite if previously indexed, omitted from `recall` and Dream consolidation, and rejected by `inspect_source`.

### Scopes, Kinds, Statuses, and Provenance

- **`ScopeTag`**: Lowercase scope tags from `aether_scope` frontmatter (defaults to `global`).
- **`AmuKind` & `AmuStatus`**: Notes carry `AmuKind` (`Profile`, `Rule`, `Decision`, `Spec` *(default)*, or `Fleeting` *(default under `Daily/` or `Journal/`)*) and `AmuStatus` (`Active` *(default)*, `Superseded`, `Archived`).
- **`SourcePointer` & `AmuId`**: Normalized vault-relative POSIX paths (`Arch/DB.md#Schema`, or `#_preamble` for text before the first ATX heading) map deterministically via SHA-256 to `AmuId` (`amu_<32-hex>`), which converts bijectively to `nytheris_aether_core::MemoryId`.

### Human-in-the-Loop Inbox (`Aether/Inbox.md`) & Compiled Mirror (`Aether/Compiled/Approved.md`)

- `VaultEngine::stage_memory` and `VaultEngine::run_dream_pass` write pending `- [ ] **[TIER | scope]** ...` entries atomically (`temp` file, `sync_all`, `rename`) to `Aether/Inbox.md` and record the file hash in `self_write_ledger` so polling skips daemon-originated writes.
- Marking an entry `[x]` in Obsidian (or calling `VaultEngine::decide_inbox_candidate` / `POST /v1/inbox/decide`) compiles it into an active AMU, materializes its `## cand_<id>` section in `Aether/Compiled/Approved.md` so `aether_inspect_source` resolves its `SourcePointer`, and marks any `supersedes_amu_id` target as `Superseded`. Marking `[-]` rejects it.

### Embedder Schema Tracking (`vault_meta`)

- `vault_meta` tracks `(schema_version, embedder_id, embedding_dim, updated_at)`. Switching embedders or dimensions on `VaultEngine::open` re-embeds active AMUs synchronously in one `IMMEDIATE` transaction when active count `<= sync_reindex_threshold` (`5000`), or populates `amus_shadow_embeddings` asynchronously and swaps atomically above threshold while keeping FTS5 + graph recall online.

### Scoped Team API Keys (`agent_api_keys`)

- `ScopedApiKey` binds a `KeyId`, an `AgentId`, an `AgentRole` (`Reader`, `Contributor`, or `Admin`), and a non-empty list of `ScopeTag` values. Plaintext secrets (`aek_<hex>`) are returned once at creation and stored as domain-separated SHA-256 digests in `agent_api_keys`, compared in constant time.
- `VaultEngine::recall_as` intersects requested scopes with the caller's allowed scopes inside `VaultStore::recall_bundle` before top-K truncation. `VaultEngine::stage_memory_as` requires `Contributor` or higher, stamps `proposed_by` with `agent.agent_id`, and rejects disallowed scopes. `VaultEngine::inspect_source_as` blocks cross-scope reads on both vault notes and `Aether/Compiled/Approved.md` entries.

### Git-Backed Team Sync & Monotonic Lattice (`git.rs`)

- `VaultEngine::git_sync` ensures `.gitignore` ignores `.aether/`, runs `git fetch` and `git merge --no-commit`, redacts any credential userinfo in `GitSyncReport` via `redact_url_userinfo`, and optionally commits and pushes merged state.
- Human notes containing unresolved Git conflict markers (`<<<<<<<`, `=======`, `>>>>>>>` outside fenced code blocks) are skipped during re-indexing so prior valid SQLite AMUs stay active until the human resolves the conflict.
- Conflicts in `Aether/Inbox.md` and `Aether/Compiled/Approved.md` are resolved deterministically by a join-semilattice keyed on `CandidateId` (`Approved > Rejected > Pending`) combined with local SQLite decision state.

### GPU KV-Tensor Warming & Eviction (`kv_cache_blocks`)

- `kv.rs` partitions `static_prefix` into chained PagedAttention prefix blocks (`KvBlockHash::paged_chained`) and `WARM`/`COLD` AMUs into CacheBlend chunks (`KvBlockHash::cache_blend`) across `GpuHbm`, `HostDram`, and `NvmeSsd`.
- `InMemoryKvCacheAdapter` tracks tiered capacity with LRU demotion (`GpuHbm -> HostDram -> NvmeSsd`), while `VllmLmCacheHttpAdapter` pushes warming payloads over HTTP (`LmCacheControl` or `OpenAiPrefill`) with offline fallback.
- Block metadata persists in SQLite (`kv_cache_blocks`), rehydrates on `VaultEngine::open`, surfaces reuse hints in `RecallResponse::kv_hint` (and MCP `structuredContent.kv_hint`), and evicts affected hashes atomically whenever AMUs are modified, removed, or superseded.
