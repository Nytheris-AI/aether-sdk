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

- **`Hot`** (`aether_tier: hot`): Packed into the static `[AETHER_HOT_PROFILE]` prefix block sorted deterministically by `amu_id` ascending within `HOT_PROFILE_MAX_TOKENS` (`2000`), with no volatile timestamps so `prefix_hash` stays stable across turns.
- **`Warm`** *(default)* (`aether_tier: warm`) and **`Cold`** (`aether_tier: cold`): Ranked dynamically per query by fusing SQLite FTS5 BM25, 256-dim signed trigram cosine similarity, and wikilink/tag `PersonalizedPageRank` (`graph_scores`) via `recall::rrf`, scaled by `authority_weight` and packed into `[AETHER_RECALL_CONTEXT]` within `max_tokens` (default `1500`).
- **`private`** (`aether_tier: private`): Exists only during parsing (`RawFrontmatterTier`) and cannot be constructed in `VaultTier`. Private notes are removed from SQLite if previously indexed, omitted from `recall`, and rejected by `inspect_source`.

### Scopes, Kinds, Statuses, and Provenance

- **`ScopeTag`**: Lowercase scope tags from `aether_scope` frontmatter (defaults to `global`).
- **`AmuKind` & `AmuStatus`**: Notes carry `AmuKind` (`Profile`, `Rule`, `Decision`, `Spec` *(default)*, or `Fleeting` *(default under `Daily/` or `Journal/`)*) and `AmuStatus` (`Active` *(default)*, `Superseded`, `Archived`).
- **`SourcePointer` & `AmuId`**: Normalized vault-relative POSIX paths (`Arch/DB.md#Schema`, or `#_preamble` for text before the first ATX heading) map deterministically via SHA-256 to `AmuId` (`amu_<32-hex>`), which converts bijectively to `nytheris_aether_core::MemoryId`.

### Human-in-the-Loop Inbox (`Aether/Inbox.md`)

- `VaultEngine::stage_memory` writes pending `- [ ] **[TIER | scope]** ...` entries atomically (`temp` file, `sync_all`, `rename`) to `Aether/Inbox.md` and records the file hash in `self_write_ledger` so polling skips daemon-originated writes.
- Marking an entry `[x]` in Obsidian compiles it into an active AMU on the next `reconcile_inbox` or `sync_vault` pass and marks any `supersedes_amu_id` target as `Superseded`. Marking `[-]` or deleting the entry rejects it.
