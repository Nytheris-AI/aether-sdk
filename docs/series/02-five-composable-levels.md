---
id: 02-five-composable-levels
slug: /series/02-five-composable-levels
sidebar_position: 2
---

# Part 2: Mastering the Five Composable Levels

In this tutorial, you run and inspect all five adoption levels of Aether SDK, from raw storage connectors up to the `MemoryManager` facade. Each level builds on the public pieces below it without hiding control flow.

**Before you begin.** Complete [Part 1: Quickstart & Your First Memory](./01-quickstart-first-memory.md) so you have a working Rust workspace checkout.

## Level 1: Direct connectors

At Level 1, you depend only on `nytheris-aether-core` and `nytheris-aether-adapters`. You call `RecordStore`, `VectorStore`, or `GraphStore` directly.

Run [`level_1_connector.rs`](https://github.com/Nytheris-AI/aether-sdk/blob/main/crates/aether-sdk/examples/level_1_connector.rs):

```bash
cargo run -p nytheris-aether-sdk --example level_1_connector
```

You see output similar to:

```text
level 1 hits: 1 (top id=<uuid>)
```

Choose Level 1 when you only need a portable `VectorStore`, `RecordStore`, or `GraphStore` client inside your own pipeline.

## Level 2: Resilience middleware

At Level 2, you wrap any connector in `Batcher`, `Timeout`, `Resilient`, and `Traced`. Because each decorator implements the same storage traits (`OutboundConnector`, `RecordStore`, `VectorStore`, `GraphStore`), callers do not change.

Run [`level_2_middleware.rs`](https://github.com/Nytheris-AI/aether-sdk/blob/main/crates/aether-sdk/examples/level_2_middleware.rs):

```bash
cargo run -p nytheris-aether-sdk --example level_2_middleware
```

Inside `level_2_middleware.rs`, the stack wraps `InMemoryBackend` and injects a simulated transient fault:

```rust
let store = Resilient::new(
    Timeout::new(Batcher::new(raw.clone()), Duration::from_secs(2)),
    RetryPolicy {
        max_retries: 3,
        base_delay: Duration::from_millis(5),
    },
);

raw.fail_next(FaultPoint::VectorUpsert, 1);
```

`Resilient` retries only errors where `AetherError::is_retryable()` is `true` (`AetherError::Transient`), using exponential backoff with full jitter (`jittered`). Wrap the outer store in `Traced::new(store)` when you want `connector_op` spans for OpenTelemetry.

You see:

```text
level 2 hits after retry: 1
```

## Level 3: Composable hybrid recall

At Level 3, you add `nytheris-aether-cortex` and assemble retrieval steps in your own code. You can insert a custom reranker, filter, or prompt formatter between any two steps.

Run [`level_3_hybrid_recall.rs`](https://github.com/Nytheris-AI/aether-sdk/blob/main/crates/aether-sdk/examples/level_3_hybrid_recall.rs):

```bash
cargo run -p nytheris-aether-sdk --example level_3_hybrid_recall
```

The example stores two records, indexes entities and a `MENTORS` relation, and fuses vector and graph results:

```rust
let vec_cands = recall::vector_search(&backend, &backend, &ns, &[1.0, 0.0], 5, None).await?;
let (graph_cands, subgraph) = recall::graph_search(
    &backend,
    &backend,
    &ns,
    &[alice.id],
    2,
    10,
    &TemporalFilter::default(),
    &PersonalizedPageRank::default(),
)
.await?;

let fused = recall::rrf(&[vec_cands, graph_cands], RRF_K);
```

`recall::vector_search` resolves hits through `RecordStore::get_many`. `recall::graph_search` pulls a bounded neighborhood, runs `PersonalizedPageRank::rank`, aggregates entity ranks onto `source_memory_id` via `graph_scores`, and resolves records. `recall::rrf` fuses both ranked lists with `RRF_K` (`60.0`).

You see:

```text
level 3 fused hits: 2, subgraph edges: 1
```

## Level 4: Durable `Indexer` sync engine

At Level 4, your code writes only to `RecordStore`. `Indexer` reads pending records and tombstones from `RecordStore`, updates the optional `VectorStore` and `GraphStore` indexes, and calls `RecordStore::mark_indexed` when the version matches.

Run [`level_4_sync_engine.rs`](https://github.com/Nytheris-AI/aether-sdk/blob/main/crates/aether-sdk/examples/level_4_sync_engine.rs):

```bash
cargo run -p nytheris-aether-sdk --example level_4_sync_engine
```

Build `Indexer` with only the derived stores you run:

```rust
let indexer = Indexer::new(backend.clone())
    .with_vector(backend.clone())
    .with_graph(backend.clone());
let report = indexer.run_pass().await?;
```

You see:

```text
level 4 indexed: 1, failed: 0, search hits: 1
```

You control when indexing runs. Call `indexer.apply(&pending)` for immediate single-record sync, call `indexer.run_pass()` on demand, or opt in to a background worker with `indexer.spawn(Duration::from_secs(5))`.

## Level 5: `MemoryManager` facade

At Level 5, `nytheris-aether-sdk::MemoryManager` combines all lower levels behind `remember`, `recall`, `update`, `forget`, `flush`, and `shutdown`. It uses only the public functions from `nytheris-aether-cortex` and `nytheris-aether-adapters`.

Run [`level_5_facade.rs`](https://github.com/Nytheris-AI/aether-sdk/blob/main/crates/aether-sdk/examples/level_5_facade.rs):

```bash
cargo run -p nytheris-aether-sdk --example level_5_facade
```

The builder wraps attached stores in `Batcher` (vector only), `Timeout` (10 seconds), `Resilient` (3 retries), and `Traced` by default:

```rust
let manager = MemoryManager::builder(backend.clone())
    .vector(backend.clone())
    .graph(backend)
    .build();
```

You see:

```text
level 5 recalled hits: 1, edges: 1
level 5 updated version: 2
```

Notice that `level_5_facade.rs` calls `manager.flush().await?` after a `WriteDurability::Buffered` write. Background indexing stays off unless you call `.background_indexing(Some(interval))` on `MemoryManagerBuilder`.

## Which level should I adopt?

| Level | Primary crates | What you own | Adopt when |
| --- | --- | --- | --- |
| Level 1 | `core`, `adapters` | Retries, timeouts, and indexing | You only want a portable single-store client |
| Level 2 | `core`, `adapters` | Sync and recall pipeline | You want `Batcher`, `Timeout`, `Resilient`, and `Traced` around raw drivers |
| Level 3 | `core`, `adapters`, `cortex` | Write path and step ordering | You want custom steps such as a reranker between `recall::*` calls |
| Level 4 | `core`, `adapters`, `cortex` | Record writes and pass schedule | You want crash-safe `RecordStore`-to-index derivation without `MemoryManager` |
| Level 5 | `sdk` | Embeddings and entity extraction | You want one lifecycle API (`remember`, `recall`, `update`, `forget`) |

You can mix levels in one binary. For example, wrap a `QdrantVectorStore` at Level 2, drive `Indexer` at Level 4, or start at Level 5 and drop down to Level 3 when a query needs custom scoring.

**Next in series.** Continue to [Part 3: Production Storage Backends](./03-production-storage-backends.md).
