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
