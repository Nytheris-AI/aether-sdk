# Scopes, Tiers & Temporal Forgetting

Multi-agent systems mix shared project knowledge, private per-user context, and short-lived conversational state. This page explains how Aether isolates those boundaries, tracks temporal validity, prevents stale vector drift on updates, and retracts forgotten facts cleanly.

## Scope isolation (`MemoryScope`)

Every `MemoryItem` belongs to one `MemoryScope` inside a `ProjectId` tenant namespace.

| Variant | Index metadata key (`index_key()`) | Storage location | Isolation mechanism |
| --- | --- | --- | --- |
| `MemoryScope::Project` (default) | `Some("project")` | `RecordStore` + derived indexes | Shared across one `ProjectId` |
| `MemoryScope::User(UserId)` | `Some("user:<id>")` | `RecordStore` + derived indexes | Filtered in `VectorStore` and `recall::in_scopes` |
| `MemoryScope::Session(SessionId)` | `None` | `RecordStore` only | TTL expiration and `RecordStore::session_items` |

### Why `User` scopes are verified twice

`VectorStore` drivers receive `_aether_scope` inside vector metadata, and `recall::scope_filter` builds a `FilterExpr::In` predicate that `MemoryManager` combines with any caller `filters` using `FilterExpr::And`. Callers cannot widen that scope predicate through `RecallQuery::filters` because caller metadata keys starting with `_aether_` are rejected by `MemoryInput::validate` and `MemoryUpdate::validate`.

`GraphStore`, however, stores entities and edges across a `ProjectId` without per-scope edge labels. A graph traversal starting at a shared entity can cross into a `MemoryItem` written under another user's scope. To close that gap structurally, `recall::graph_search` resolves `source_memory_id` records via `RecordStore::get_many` and passes candidates through `recall::in_scopes`, dropping any memory outside `RecallQuery::scopes` before returning results.

### Why `Session` memories never enter derived indexes

Conversational working memory has a short lifetime. Writing session turns into external vector or graph stores would require background sweep jobs to delete expired vectors across Qdrant, pgvector, Chroma, and Neo4j. Without constant sweeping, expired turns would remain searchable indefinitely.

Instead, `MemoryScope::Session(SessionId)` returns `None` from `index_key()`. `Indexer::apply` marks session records indexed without writing to `VectorStore` or `GraphStore`. `RecordStore::create` stores session items with a TTL (`DEFAULT_SESSION_TTL` is `3600` seconds in `MemoryManager`, configurable via `with_session_ttl`), and `RecordStore::session_items` filters expired rows out at read time while `recall::session` scores active ones in process.

## Memory tiers (`MemoryTier`)

`MemoryTier` has four variants (`Episodic`, `Semantic` as default, `Procedural`, and `Profile`). In the core SDK, `MemoryTier` carries no hidden eviction or routing rules of its own. `index_metadata` writes it to vector metadata as `_aether_tier` so your queries can filter by tier using `FilterExpr::Eq` or `FilterExpr::In` without changing schema.

In `nytheris-aether-vault`, by contrast, `VaultTier` (`Hot`, `Warm`, `Cold`) directly drives context placement because `Hot` entries assemble into the static `[AETHER_HOT_PROFILE]` prefix block while `Warm` and `Cold` entries compete dynamically for the remaining token budget.

## Temporal validity and point-in-time queries

Graph facts change over time without erasing history. Every `Relation` and `GraphEdge` carries `valid_at: DateTime<Utc>` and `invalid_at: Option<DateTime<Utc>>`.

`GraphStore::neighborhood` accepts a `TemporalFilter`:

- `active_only: true` (default) excludes edges where `invalid_at.is_some()`.
- `as_of: Some(ts)` selects edges where `valid_at <= ts` and `invalid_at` is either `None` or strictly greater than `ts`.

In `nytheris-aether-vault`, AMUs also carry `valid_from` and `superseded_by: Option<AmuId>` alongside `AmuStatus` (`Active`, `Superseded`, `Archived`). Approving an Inbox entry that specifies `supersedes_amu_id` marks the older AMU as `Superseded` and stamps its `superseded_by` column in the same SQLite transaction.

## The `update` re-embedding invariant

When calling `MemoryManager::update` on an existing `MemoryItem` that already has an `embedding`, changing `content` without providing a new `embedding` fails with `AetherError::InvalidInput`.

The alternative would be keeping the old vector beside new text or silently deleting the vector from the index. Both alternatives cause recall bugs because Aether stays model-free and cannot re-embed text itself. Rejecting content-only updates on embedded records forces callers to keep text and vectors aligned at the boundary.

## Forgetting and `retract_memory_facts` provenance

Every `GraphEdge` written to `GraphStore` stores `source_memory_id: MemoryId`, keyed idempotently by `(source_memory_id, source, target, relation_type)`. Tracking provenance at the edge level prevents one memory from deleting shared facts asserted by other memories.

When `MemoryManager::forget` runs:

1. `RecordStore::tombstone` marks the record deleted and bumps its version, hiding it immediately from `get`, `get_many`, and `pending_for`.
2. `Indexer::apply` deletes the vector from `VectorStore` and calls `GraphStore::retract_memory_facts(ns, id, Utc::now())`, which stamps `invalid_at` on edges owned by that `source_memory_id`.
3. Only after both index operations succeed does `RecordStore::mark_indexed` permanently remove the tombstoned record.

If `VectorStore::delete` or `GraphStore::retract_memory_facts` fails due to a transient outage, `forget` returns the error, the record stays invisible to reads, and the tombstone remains pending in `RecordStore::pending` so the next `flush()` or `Indexer::run_pass()` finishes cleaning up the derived indexes.
