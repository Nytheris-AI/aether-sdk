# System of Record, Derived Indexes & Consistency

Keeping text records, vector embeddings, and graph edges consistent across crashes and retries is harder than calling three databases in sequence. This page explains why Aether makes `RecordStore` the authoritative source of truth and derives `VectorStore` and `GraphStore` from versioned record state.

## Why dual writes fail without an authoritative record

If an application writes directly to a record database, a vector database, and a graph database, any partial failure splits state. If a crash happens after vector upsert and before record creation, searches return orphaned IDs with no body. If a hard delete fails halfway through, forgotten memories come back in later vector queries.

Earlier gateway designs tried to guard this with in-process pending queues, hot caches, and write-ahead logs. That meant three separate structures each held copies of recent writes, and an in-process crash still dropped buffered items.

Aether removes those in-process copies by placing durability state directly inside `RecordStore`.

## `RecordStore` durability mechanics

Every live memory record in `RecordStore` carries a monotonically increasing `version` (`u64`), an `indexed_version` (`u64`), and a `tombstone` flag. Four operations make consistency structural rather than best-effort.

### Atomic idempotency on `create`

When `MemoryInput::idempotency_key` is set, `RecordStore::create` stores the `MemoryItem` and its idempotency key in one atomic operation scoped by `ProjectId`. Repeating `create` with the same `(project_id, idempotency_key)` returns `CreateOutcome::Duplicate(MemoryReceipt)` instead of creating a duplicate record.

The receipt's `ReceiptState` (`Buffered` or `Persisted`) reflects whether `indexed_version == version`. Once the record is tombstoned or its session TTL expires, the idempotency key is released with it.

### Optimistic concurrency on `cas_update`

`RecordStore::cas_update` takes an `expected_version: Option<u64>`. When provided, the backend verifies that the stored record is live and still at `expected_version` before writing `version + 1`. A mismatch returns `AetherError::Conflict { memory_id, expected_version, actual_version }`, preventing concurrent writers from silently overwriting each other's updates.

### Versioned `tombstone` deletes

`RecordStore::tombstone` does not immediately purge the row. Instead, it marks the record as tombstoned and bumps `version` in one atomic step, returning the prior `MemoryItem`.

From that moment onward, `get`, `get_many`, and `pending_for` exclude the tombstoned record so forgotten data disappears from recall immediately. Meanwhile, `RecordStore::pending` continues emitting the item as `PendingRecord { item, tombstone: true }` until derived indexes remove its vector and retract its graph edges.

### Version-gated `mark_indexed`

After updating derived indexes, `Indexer::apply` calls `RecordStore::mark_indexed(ns, id, version)`. Backends apply a strict version guard:

- If the stored `version` still equals `version` and `tombstone` is `false`, `indexed_version` advances to `version`.
- If the stored `version` still equals `version` and `tombstone` is `true`, the record and its idempotency key are deleted permanently.
- If a concurrent `cas_update` or `tombstone` bumped the record's version while indexing was in flight, `mark_indexed` becomes a no-op so the newer version stays pending for the next pass.

This version guard also prevents resurrection when `forget` races with an in-flight write pass.

## Crash-safe `Indexer` derivation

`muninn_aether_cortex::Indexer` reconciles `RecordStore` into optional `VectorStore` and `GraphStore` connectors.

- **Single-record application (`Indexer::apply`).** For a live non-session record, `apply` upserts the vector (or deletes any old vector if `embedding` is `None`), retracts previous graph facts for `item.id`, upserts current `entities` and `relations`, and calls `mark_indexed`. For a tombstone, it deletes the vector, retracts graph facts, and calls `mark_indexed`.
- **Batched sweeps (`Indexer::run_pass`).** `run_pass` pulls batches of up to `256` records from `RecordStore::pending`. If applying one record returns an error, `Indexer` records the failure in `PassReport::failed`, tracks that `MemoryId` for the rest of the pass so it does not spin in a tight loop, and continues processing other records. Any unindexed or failed records remain pending in `RecordStore` across process restarts.
- **Opt-in background loop (`Indexer::spawn`).** Calling `spawn(interval)` starts a `tokio` task woken either by `Indexer::notify()` on buffered writes or by the fallback `interval` timer. Dropping all strong `Arc<Indexer>` handles terminates the loop cleanly.

Because vector upserts, vector deletes, graph relation upserts, and graph retractions are all idempotent, two processes running `Indexer::run_pass` concurrently over the same store converge safely.

## `WriteDurability` and `ConsistencyMode` trade-offs

Callers choose per-write durability and per-query consistency explicitly without changing store topology.

| Setting | Option | Behavior and cost |
| --- | --- | --- |
| `WriteDurability` | `Buffered` (default) | Persists to `RecordStore`, wakes `Indexer`, and returns `ReceiptState::Buffered`. Lowest write latency. |
| `WriteDurability` | `WaitFlush` | Persists to `RecordStore` and runs `Indexer::apply` before returning `ReceiptState::Persisted`. Higher write latency, immediate index sync. |
| `ConsistencyMode` | `ReadYourWrites` (default) | Queries `VectorStore` plus `RecordStore::pending_for` scored via `DistanceMetric::score`, keeping newest `version` with `recall::merge`. |
| `ConsistencyMode` | `Eventual` | Queries derived indexes (plus explicit `Session` scopes) without scanning `pending_for`. Faster when write-to-read lag is acceptable. |

Notice why `ReadYourWrites` stays accurate even before indexing finishes. `recall::unindexed` inspects `VectorStore::describe` to fetch the exact `DistanceMetric` of the target index and scores pending embeddings with `DistanceMetric::score`, avoiding arbitrary hardcoded placeholder scores. And because `recall::vector_search` and `recall::graph_search` always resolve hits through `RecordStore::get_many`, stale or tombstoned index entries are dropped at read time before reaching your application.
