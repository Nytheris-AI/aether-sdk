# Aether SDK Roadmap

Aether SDK is built as a unified Rust **AI Memory Gateway** for connecting applications to AI memory storage and databases (**Redis/Valkey, SQLite, Turbopuffer, Neo4j, Qdrant, Chroma, pgvector**).

## Current State

The four crates exist, compile, and pass their tests against `InMemoryBackend`.
The shape of the system is in place, but several behaviours the docs describe are not implemented yet, and a few of the implemented ones are unsafe under failure.
The table below is the honest status, not the aspirational one.

| Area | Status | Notes |
|------|--------|-------|
| `aether-core` contracts | Done | Newtypes, DTOs, `FilterExpr`, `TemporalFilter`, `AetherError`, object-safe traits. |
| `InMemoryBackend` + contract suite | Done | Only backend that exists. The suite is not yet reusable against a second backend. |
| Middleware (`Traced`, `Resilient`, `Timeout`, `Batcher`) | Partial | No jitter, no circuit breaker. `MemoryManager` applies only `Traced` by default. |
| `PolicyRouter` | Partial | Validation and idempotency only. No embedding-dimension check. |
| `IngestionBuffer` | Partial | No background worker, unbounded, loses data on a failed flush. |
| `QueryPlanner` | Partial | Vector search plus read-your-writes merge. Graph results are returned but never influence ranking. |
| `MemoryManager` | Partial | Full lifecycle API works on the happy path. Failure and race paths are listed below. |
| Observability | Done | `tracing` span hierarchy and the `otel` preset. |
| Production drivers | Not started | Feature flags exist with no code behind them. |
| Rust CI | Not started | CI still runs the retired Python toolchain only. |

## Phase 5 - Correctness Hardening (next)

The sections below list the problems.
The [Phase 5 design](../architecture/hardening-design.md) resolves them into one design and an order of work.

This phase comes before any production driver.
Every driver written now would inherit the semantics below and make them more expensive to change.
Each item is a behaviour change and follows Red - Green - Refactor with a failing test first.

### 5.1 Decide the system of record

Today `KvStore` is described as an "L2 cache" but is actually the only place a complete `MemoryItem` lives.
Non-session items are written to it with no TTL, `update()` reads from it, and memories without an embedding never reach L3 at all.
The vector store only holds a copy of `content` stuffed into its metadata.

Proposal: make this explicit instead of accidental.

- The `KvStore` role becomes the **record store**: the durable source of truth for every `MemoryItem`.
- `VectorStore` and `GraphStore` become **derived indexes** that can be rebuilt from the record store.
- Recall resolves hit content from the record store by `MemoryId`, so drivers stop carrying `content` in vector metadata.
- The "needs indexing" state lives in the record store, which gives crash-safe write-behind without a separate `WalStore`.

This removes `WalStore` from the plan and makes Redis a poor default for this role (SQLite or Postgres fit better).
Session-scoped working memory with TTL stays as it is.

### 5.2 Write path

- **Failed flush loses data.**
  `IngestionBuffer::flush` drains the queue before writing, so one failed `upsert_batch` drops every drained item for every project.
  Items must only leave the pending set after their index writes succeed.
- **`Buffered` writes are never flushed on their own.**
  There is no worker, so a `Buffered` write reaches L3 only when someone else calls `flush()` or issues a `WaitFlush` write.
  Add a background flusher with a size trigger and an interval trigger.
- **The queue is unbounded.**
  Add a capacity with backpressure (`remember` waits or returns `Transient`).
- **Two idempotency mechanisms, one of them leaks.**
  `PolicyRouter::seen_keys` grows forever and records the key before the item is stored, so a failed `remember` permanently maps the key to a memory that does not exist.
  Keep only `KvStore::put_if_absent`, and write the key and the item atomically.
- **Dimension mismatches are accepted, then fail later.**
  A wrong-dimension embedding passes `PolicyRouter`, fails inside the driver at flush time, and (via the first bullet) takes the rest of the batch with it.
  Validate dimensions at the inbound boundary.
- **`update()` can leave a stale vector.**
  Changing `content` without a new `embedding` keeps the old vector indexed against the new text.
  Either require an embedding when content changes or drop the item from the vector index until one is supplied.
- **`update()` cannot change entities or relations.**
  Add them to `MemoryUpdate`, retracting the previous version's facts.

### 5.3 Forget path

- **A forgotten memory can come back.**
  `forget` removes the item from the pending queue, but a flush that already drained it will upsert it after the delete.
  Serialize `forget` against flush per memory, or tombstone in the record store and have flush skip tombstoned ids.
- **Partial failure orphans index entries.**
  `forget` deletes the record first, so a failed vector delete leaves an orphan with nothing left to retry from.
  Delete the indexes first and the record last.

### 5.4 Recall path

- **Read-your-writes scores are invented.**
  Unflushed items get a constant `0.75` or `0.5` and are sorted together with real cosine scores, so an unrelated recent write can outrank a genuine match.
  Score unflushed items with the same metric as the index, and leave out items that cannot be scored.
- **Expired session items are still served.**
  `HotContextCache` ignores TTL, so a session memory that expired in L2 is still returned from L1.
- **Hybrid recall does not fuse anything.**
  `PersonalizedPageRank` runs and its result is discarded.
  Either use the ranking to score memories through their `source_memory_id` edges, or remove the PPR call until it is used.
- **Seed resolution does not match the design.**
  The blueprint says seeds are supplemented from top-k vector hits.
  The code only substring-matches entity names in the local pending queue and hot cache, which does nothing on a fresh process.

### 5.5 Scopes and tiers

- **`MemoryScope::User` and `Session` carry no identity.**
  There is no user id or session id, so two users in one project share one "user scope".
  Scopes need to carry their subject and recall needs to filter on it.
- **`MemoryTier` is stored and never read.**
  Either give tiers behaviour (routing, TTL, ranking) or document them as a plain label.

### 5.6 Middleware and defaults

- `MemoryManager::builder` should apply `Timeout` and `Resilient` by default, not only `Traced`.
- Add jitter to retry backoff.
- The circuit breaker described in the connectors doc does not exist.
  Drop it from the docs until an outage shows it is needed.

### 5.7 Repository hygiene

- Add a Rust CI job: `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test --workspace`.
- Retire the Python package (`src/`, `tests/`, `pyproject.toml`, the Python CI job) and rewrite `README.md`, which still documents the Python API.
- Bring `docs/development/guidelines.md` in line with the Rust codebase.

## Phase 6 - First Production Backends

Build one backend per role and prove the contract suite against real infrastructure before adding more.

- **Record store**: `sqlite` (WAL mode).
- **Vector index**: one of `qdrant` or `pgvector`.
- **Graph index**: `neo4j`.
- Make the contract suite generic over a backend factory so `InMemoryBackend` and every driver run the identical tests.

The remaining drivers (`redis`, `turbopuffer`, `chroma`) follow only after the first three have run the suite unchanged.
A driver that needs a contract change is a signal the contract is wrong, and that is cheapest to learn with three drivers, not seven.

## Phase 7 - Bindings

- Optional `PyO3` Python bindings.
- HTTP/gRPC gateway server.
- MCP server.

## Scope Decision

Decided 2026-11-16: Aether stays a storage gateway.
It stores, indexes, and retrieves what the caller hands it, and does not embed, chunk, extract, or call models.
The value it competes on is keeping a record store, a vector index, and a graph index consistent for one memory through writes, updates, deletes, crashes, and retries.
Model-backed features (text-in recall, consolidation, context assembly) are deliberately deferred until that core has users.
