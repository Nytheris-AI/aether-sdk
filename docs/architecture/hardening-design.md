# Phase 5 Design - Gateway Hardening

Status: **draft, awaiting approval**.
This document resolves the open choices in [Roadmap Phase 5](../about/roadmap.md) into one design a builder can plan from.

## 1. Intent

Aether stays a **storage gateway** (decided 2026-11-16).
It does not embed, chunk, extract, or call models.
The audience is developers who run their own memory, RAG, or knowledge-base stack and already produce their own embeddings and entities.

The specific problem Aether solves for them:

> One memory lives in several stores (a record, a vector, graph facts).
> Keeping those stores consistent through writes, updates, deletes, crashes, and retries is the part every team rebuilds badly.
> Aether makes it one call with stated guarantees.

Everything in this phase serves that sentence.
A feature that does not make the multi-store guarantee stronger or easier to trust is out of scope.

## 2. Goals, Non-Goals, Anti-Goals

**Goals**

- G1. An acknowledged write is never lost, including across a process crash.
- G2. A forgotten memory never reappears and leaves no orphaned index entries.
- G3. A caller reads its own writes with scores that mean the same thing as indexed scores.
- G4. Expired and out-of-scope memories are never returned.
- G5. Bad input fails at the inbound boundary, not later inside a driver.
- G6. Every guarantee above is expressed as a contract test that any backend must pass.

**Non-goals**

- Embedding, chunking, extraction, summarization, consolidation, or any model call.
- New production drivers (Phase 6).
- Cross-store distributed transactions.
  Indexes are eventually consistent with the record store by design.

**Anti-goals**

- A guarantee that holds only for `InMemoryBackend`.
- Silent degradation: returning an empty or approximate result where an error is the honest answer.

## 3. Approaches Considered

**A. Patch each bug in place.**
Keep the in-process queue, hot cache, and `KvStore`-as-cache, and fix the listed defects one by one (requeue on failure, lock `forget` against flush, add TTL to the cache).
Smallest diff per fix.
It keeps three places that each hold "recent writes" (queue, cache, KV) and must be kept in agreement, and it cannot deliver G1 across a crash without adding a separate write-ahead log.

**B. Record store as the source of truth, indexes derived (chosen).**
Every write, update, and delete is a versioned change to one durable record.
A single indexer brings the vector and graph indexes up to the record's version.
Most of the listed defects stop being possible instead of being individually guarded, and three components are deleted.
Cost: the storage trait changes shape, and the record-store role needs a real database (SQLite, Postgres) for the durability claim.

**C. Outbox with a separate write-ahead log.**
Keep `KvStore` as a cache and add `WalStore` as the durable log.
Delivers G1, but adds a fourth storage role and a second ordering to reconcile with the KV state.

B was chosen because it is the only option where the guarantees follow from the structure.
It was the proposal in roadmap section 5.1.

## 4. Design

### 4.1 Roles

| Role | Trait | Holds | Truth? |
|------|-------|-------|--------|
| Record store | `RecordStore` (renamed from `KvStore`) | Complete `MemoryItem`, version, indexing state, tombstones, idempotency keys | Yes |
| Vector index | `VectorStore` | `id`, vector, filterable metadata | Derived |
| Graph index | `GraphStore` | Entities and provenance-tagged relations | Derived |

The vector index no longer carries `content`.
Recall resolves hits through the record store by `MemoryId`.
A hit whose record is missing, tombstoned, or expired is dropped, which is what makes G2 and G4 structural.

### 4.2 `RecordStore` contract

```rust
#[async_trait]
pub trait RecordStore: OutboundConnector {
    /// Stores the item and, if present, its idempotency key in one atomic step.
    async fn create(&self, ns: &ProjectId, item: &MemoryItem, ttl: Option<Duration>) -> Result<CreateOutcome>;
    async fn get(&self, ns: &ProjectId, id: MemoryId) -> Result<Option<MemoryItem>>;
    async fn get_many(&self, ns: &ProjectId, ids: &[MemoryId]) -> Result<Vec<MemoryItem>>;
    async fn cas_update(&self, ns: &ProjectId, id: MemoryId, expected_version: Option<u64>, new_item: &MemoryItem) -> Result<()>;
    /// Marks the record deleted, bumps its version, and returns it. `None` if there is no live record.
    /// The record is removed by `mark_indexed`.
    async fn tombstone(&self, ns: &ProjectId, id: MemoryId) -> Result<Option<MemoryItem>>;

    /// Records whose indexes are behind their version, oldest first, across all projects.
    async fn pending(&self, limit: usize) -> Result<Vec<PendingRecord>>;
    /// Same, for one project. Used by read-your-writes.
    async fn pending_for(&self, ns: &ProjectId, limit: usize) -> Result<Vec<MemoryItem>>;
    /// Live items in one session scope. Used to recall working memory.
    async fn session_items(&self, ns: &ProjectId, session: &SessionId, limit: usize) -> Result<Vec<MemoryItem>>;
    /// No-op unless `version` is still the record's current version. Removes the record if it is a tombstone.
    async fn mark_indexed(&self, ns: &ProjectId, id: MemoryId, version: u64) -> Result<()>;
}

pub enum CreateOutcome { Created, Duplicate(MemoryReceipt) }
pub struct PendingRecord { pub item: MemoryItem, pub tombstone: bool }
```

`put`, `put_if_absent`, `recent`, and `delete` are removed.
`pending` is the only method that crosses project boundaries, and it is called only by the indexer.
An idempotency key lives exactly as long as its record and is removed with it.

### 4.3 Write, update, forget: one pipeline

All three become "change the record, then index".

1. **remember**: validate, then `create`.
   `Duplicate` returns the existing receipt.
2. **update**: read, apply patch, `cas_update` with `version + 1`.
3. **forget**: `tombstone`.
4. In every case the record is now pending.
   `Buffered` returns immediately.
   `WaitFlush`, `update`, and `forget` index that record before returning.
   If indexing that record fails, the waiter receives the error and the record stays pending for retry.

**Indexer** (replaces `IngestionBuffer`):

- Loop: `pending(batch)`, then for each record apply it to the indexes, then `mark_indexed(id, version)`.
- Live record: upsert the vector if there is an embedding, otherwise delete any vector for that id; retract the memory's previous graph facts, then upsert current entities and relations.
- Upserting relations for a memory after its facts were retracted must leave those facts active.
  This is added to the `GraphStore` contract.
- Tombstone: delete the vector, retract graph facts, then `mark_indexed` removes the record.
- A failed record stays pending and is retried on the next pass.
  One bad record does not block or drop the others.
- Woken by a notify on every write, and by an interval timer as a fallback.
- Index operations are idempotent upserts and deletes, so two replicas processing the same record is harmless.
- `mark_indexed` ignores stale versions, so a record updated mid-pass simply stays pending.

What this removes:

| Removed | Why it is no longer needed |
|---------|----------------------------|
| In-process pending queue | Pending state is in the record store. Fixes data loss on failed flush and on crash. |
| `HotContextCache` | Its only users were read-your-writes and seed lookup, both now served by `pending_for`. Removes the TTL leak. Reintroduce only if measured read latency calls for it. |
| `PolicyRouter` | Idempotency moves into `create`; validation is `MemoryInput::validate` plus the dimension check. Removes the unbounded key map. |
| `WalStore` (planned) | The record store is the log. |
| Queue capacity and backpressure (planned) | There is no in-memory queue to bound. |

### 4.4 Validation at the boundary

- `VectorStore` gains `describe(ns) -> Result<Option<IndexSpec>>` where `IndexSpec { dimensions, metric }`.
- `remember`, `update`, and `recall` check vector length against it and return `InvalidInput` on mismatch.
- `update` that changes `content` must also supply `embedding` if the item has one.
  The gateway cannot re-embed, and a vector that no longer matches its text is worse than an error.
- `MemoryUpdate` gains `entities` and `relations`.
- `recall` in `Vector` or `Hybrid` mode without `query_vector` returns `InvalidInput`.
  `Graph` mode without seeds returns `InvalidInput`.
- Metadata keys starting with `_aether_` are reserved and rejected in caller input.

### 4.5 Scopes, tiers, and sessions

```rust
pub enum MemoryScope { Project, User(UserId), Session(SessionId) }
```

- `Global` is removed.
  It was documented as "shared within a project", which is `Project`.
- `RecallQuery` gains `scopes: Vec<MemoryScope>`, defaulting to `[Project]`.
  A typical agent turn passes `[Project, User(u), Session(s)]`.
- The gateway writes `_aether_scope` and `_aether_tier` into vector metadata and ANDs a scope filter into every search.
  Callers cannot widen it.
- **Session memories are never written to the indexes.**
  They live only in the record store with a TTL, and `mark_indexed` is called for them without index writes.
  At recall, `session_items` for each requested session are scored in process.
  This fixes expired session memories remaining searchable in the vector store forever.
- `MemoryTier` stays a label with no behaviour, filterable through `_aether_tier`.
  The memory guide will say so.

### 4.6 Recall

1. Validate (4.4).
2. Vector search with the scope filter and caller filter, `top_k`.
3. Resolve hits through `get_many`; drop the unresolved.
4. If `ReadYourWrites`: add `pending_for` items that match scope and filter.
   If `Eventual`: skip.
5. Add `session_items` for requested sessions.
6. Items from steps 4 and 5 are scored against `query_vector` using the index's metric from `describe`.
   Items with no embedding are left out of `Vector` and `Hybrid` ranking.
   The constant `0.75` and `0.5` scores are deleted.
7. `Hybrid` and `Graph`: seeds are the caller's `seed_entities` plus the entities of the resolved vector hits.
   Fetch the neighbourhood, run PPR, and give each memory the sum of PPR scores of entities on edges it sourced (`source_memory_id`).
8. `Hybrid` fuses the vector ranking and the graph ranking with reciprocal rank fusion.
   Graph-only memories are resolved through `get_many` like any other hit.
9. The graph index carries no scope, so every resolved item is checked against the requested scopes in the gateway before it is returned.

### 4.7 Middleware defaults

- `MemoryManager::builder` wraps each store as `Traced<Resilient<Timeout<T>>>`, with `Batcher` innermost for the vector store.
  Each layer can be switched off on the builder.
- Retry backoff gets full jitter.
- The circuit breaker is removed from the connectors doc.

### 4.8 Contract suite

- The suite becomes generic over a backend factory so Phase 6 drivers run it unchanged.
- New cases, one per guarantee: failed index write keeps the record pending (G1); pending records survive a rebuilt manager over the same store (G1); forget during an in-flight index pass does not resurrect (G2); failed index delete is retried and leaves no orphan (G2); unflushed and flushed copies of the same item score equally (G3); expired session item is not recalled (G4); user A cannot recall user B's memories (G4); wrong-dimension write is rejected at `remember` (G5).
- A fault-injecting wrapper around `InMemoryBackend` drives the failure cases.

### 4.9 Repository hygiene

- Rust CI: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`.
- Remove the Python package, its tests, `pyproject.toml`, and the Python CI job.
  Rewrite `README.md` and `docs/development/guidelines.md` for Rust.
  This deletes code and needs explicit sign-off before it is done.

## 5. Order of Work

Each step is a series of micro commits, test first, and leaves the workspace green.

1. Rust CI, so every later step is checked.
2. Rename `KvStore` to `RecordStore` (structural only).
3. Fault-injecting test backend and generic contract suite.
4. New `RecordStore` methods and the indexer; delete the queue, `HotContextCache`, and `PolicyRouter`.
5. Forget through tombstones.
6. `describe` and boundary validation; `MemoryUpdate` entities and relations.
7. Scopes with identity; session memories record-only.
8. Recall: resolve through records, real scores, seeds from hits, PPR fusion.
9. Middleware defaults and jitter.
10. Docs: memory guide, usage guide, connectors, architecture diagram, README.
11. Python removal (after sign-off).

## 6. Production Impact

None on running systems: no production driver exists and nothing is published.
The public Rust API changes in breaking ways (`KvStore` renamed and reshaped, `MemoryScope` variants, `MemoryUpdate` fields, `recall` now errors where it returned empty results).
The crate is at `0.1.0` and unreleased, so no migration path is provided.

## 7. Calls Made Here (not yet confirmed)

These were chosen by the author and are open to correction.

- Renaming `KvStore` to `RecordStore` instead of only redefining it in prose.
- Deleting `HotContextCache` and `PolicyRouter` instead of fixing them.
- `update` with new content and no new embedding is an error, not a silent removal from the index.
- `recall` without a usable query is an error, not "most recent items".
- Session memories are scored in process and never indexed.
- `Global` scope removed.
- Reciprocal rank fusion for hybrid ranking.
- `MemoryTier` stays a label.

## 8. Left to the Builder

- SQL or key layout for pending state and idempotency keys inside each backend.
- Indexer batch size, interval, and how waiters are notified of a specific version.
- RRF constant and PPR parameters, kept as named defaults.
- Whether `session_items` needs pagination (start without).
