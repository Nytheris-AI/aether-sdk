# Change Overview - From Gateway to Pieces-First SDK

Status: **implemented** (2026-11-18). All code changes below have landed across the Rust workspace.
This page is the one-screen summary of everything that will change in the code, the docs, and the roadmap.
The reasoning lives in the [pieces-first SDK design](./sdk-design.md) and the [Phase 5 hardening design](./hardening-design.md).
Where this page and those documents disagree, they win and this page has a bug.

## 1. In One Paragraph

Aether stops being one facade that every call must pass through and becomes a set of pieces a developer calls from their own workflow.
The record store becomes the source of truth and the vector and graph stores become indexes derived from it.
Three in-process components that each held their own copy of "recent writes" are deleted, the single `QueryPlanner` becomes a handful of public recall steps, and `MemoryManager` shrinks to an optional convenience built only from those public pieces.
Nothing is released and no production driver exists, so there is no migration and no production impact.

## 2. Before and After

| | Today | After |
|---|-------|-------|
| Entry point | `MemoryManager` only | Any piece, at any of five levels; `MemoryManager` optional |
| Source of truth | Unclear: KV "cache" in practice, vector metadata holds a copy of content | The record store, explicitly |
| Stores required | Record, vector, and graph, always | Only the ones in use (the facade needs a record store) |
| Pending writes | In-process queue, lost on crash or failed flush | State in the record store, retried until indexed |
| Background work | None exists, though docs imply it | None unless the developer opts in |
| Recall | One sealed `QueryPlanner` | Public steps the developer can reorder and interleave |
| Unflushed-item scores | Constants `0.75` and `0.5` | The index's own metric |
| Scopes | Labels with no identity | `User(UserId)`, `Session(SessionId)`, enforced |
| Extension | None | Composition: put your step between two Aether steps |
| Transports | Trait kept ready for HTTP, gRPC, MCP | None planned |

## 3. What Gets Removed

### 3.1 Code that exists today

| Remove | Crate | Why | Replaced by |
|--------|-------|-----|-------------|
| `MemoryInbound` trait | `muninn-aether-core` | Existed for server transports that are no longer planned; one implementation | Inherent methods on `MemoryManager` |
| `PolicyRouter`, `PolicyDecision` | `muninn-aether-cortex` | Unbounded in-memory key map; records a key before the item is stored | `RecordStore::create` (atomic idempotency) and `MemoryInput::validate` |
| `HotContextCache` | `muninn-aether-cortex` | Third copy of recent writes; serves expired session items | `RecordStore::pending_for` and `session_items` |
| `IngestionBuffer` | `muninn-aether-cortex` | Drains before writing, so a failed flush or a crash loses data | `Indexer` over pending records |
| `QueryPlanner` | `muninn-aether-cortex` | Sealed pipeline with no place for the developer's own step | `recall::*` steps |
| `enrich_metadata` | `muninn-aether-cortex` | Copies `content` and `version` into vector metadata | `index_metadata` (no content; adds `_aether_scope`, `_aether_tier`) |
| Constant read-your-writes scores `0.75` / `0.5` | `muninn-aether-cortex` | Not comparable with real similarity scores | `DistanceMetric::score` |
| Entity-name substring seed lookup | `muninn-aether-cortex` | Only sees local unflushed items; does nothing on a fresh process | Seeds from caller plus entities of vector hits |
| `cosine`, `eval_filter`, `value_matches` | `muninn-aether-cortex` | Duplicates of the same logic in the in-memory backend | `DistanceMetric::score`, `FilterExpr::matches` in `muninn-aether-core` |
| `similarity`, `eval_filter`, `value_matches` | `muninn-aether-adapters` | Same duplication, other side | Same |
| `RecordStore::put`, `put_if_absent`, `recent`, `delete` | `muninn-aether-core` and every implementor | Non-atomic idempotency, no pending state, hard delete that can orphan indexes | `create`, `pending_for`, `tombstone` |
| `MemoryScope::Global` | `muninn-aether-core` | Documented as "shared within a project", which is `Project` | `MemoryScope::Project` |
| `MemoryManager::new(kv, vector, graph)` | `muninn-aether-sdk` | Forces all three stores | `MemoryManager::builder(records)` with `.vector(..)` and `.graph(..)` |
| Empty feature flags `turbopuffer`, `qdrant`, `neo4j`, `chroma`, `pgvector` | `muninn-aether-adapters` | Advertise drivers that do not exist | Each flag returns with its driver in Phase 6 |
| The word "gateway" in crate docs and comments | all crates | No longer what Aether is | "SDK", "piece", or the specific component name |

### 3.2 Things planned in the docs that will not be built

| Drop from plans | Where it was described | Why |
|-----------------|------------------------|-----|
| `WalStore` | Connectors doc | The record store is the log |
| `WorkerPool` | Rust migration blueprint | `Indexer::run_pass`, driven by the developer or by the opt-in `Indexer::spawn` |
| L1 cache on `moka` | Connectors doc | No L1 cache at all until measured read latency asks for one |
| Queue capacity and backpressure | Roadmap 5.2 | There is no in-memory queue left to bound |
| Circuit breaker in `Resilient` | Connectors doc | Never existed; add it when an outage shows the need |
| `config.rs` and `ConnectorRegistry` | Connectors doc | A config-driven registry that builds and wraps drivers is a gateway feature; developers construct the stores they use |
| HTTP/gRPC gateway server | Roadmap, blueprint | Aether is a library, not a service |
| MCP server | Blueprint | Same |

Python bindings are deferred, not dropped.

### 3.3 Repository contents

These are removed in the last step of the work and **need explicit owner sign-off first**.

| Remove | Why |
|--------|-----|
| `src/muninn_aether_sdk/` (Python package) | Retired implementation; the product is the Rust workspace |
| `tests/` (Python tests) | Tests for the retired package |
| `pyproject.toml` | Packaging for the retired package |
| `examples/quickstart.py` | Python example for the retired API |
| Python job in `.github/workflows/ci.yml` | Nothing left for it to test |
| Python content of `README.md` | Documents an API that no longer exists |
| Python content of `docs/development/guidelines.md` (about 1,800 lines) | Python style guide for the retired package |

### 3.4 Documentation

| Remove | Why |
|--------|-----|
| `docs/architecture/rust-migration.md` | A migration blueprint for a gateway. Its contracts section is replaced by the two design docs and by rustdoc; its phase list is already replaced by the roadmap |
| "L1 / L2 / L3" vocabulary | Describes a cache hierarchy that no longer exists; the terms become record store, vector index, graph index |
| "Gateway" positioning on every page | Replaced by "memory SDK" |
| Roadmap sections 5.1 to 5.6 as a problem list | Once fixed, they collapse into the completed-milestones list |

## 4. What Changes

| Change | From | To |
|--------|------|----|
| Storage trait name | `KvStore` | `RecordStore` |
| Storage trait role | L2 cache and working memory | Durable source of truth for every `MemoryItem` |
| `VectorStore` | Holds vector plus a copy of content | Holds vector plus filterable metadata only; gains `describe` |
| `GraphStore::upsert_relations` | Appends a new edge every call | Idempotent per (memory, source, target, relation type) |
| `MemoryScope` | `Copy` enum of four labels | `Project`, `User(UserId)`, `Session(SessionId)` |
| `MemoryUpdate` | Content, embedding, metadata | Also entities and relations |
| `RecallQuery` | No scope, silently returns nothing useful without a vector | Has `scopes`; invalid queries return `InvalidInput` |
| `forget` | Hard delete in three stores, in an order that can orphan | Tombstone, then the same indexing pipeline as a write |
| `update` with new content and no new embedding | Keeps the stale vector | `InvalidInput` |
| Session memory | Written to vector and graph stores, where it outlives its TTL | Record store only, scored at recall time |
| `Buffered` write | Sits in memory until someone flushes | Durable immediately, visible to recall, indexed on `flush()` or by the opt-in loop |
| Hybrid recall | Vector hits; graph result returned but unused | Vector and graph rankings fused with reciprocal rank fusion |
| `MemoryManager` builder defaults | `Traced` only | `Traced`, `Resilient`, `Timeout`, each switchable |
| Retry backoff | Exponential, no jitter | Exponential with full jitter |
| `Traced` span op names | `kv_*` | `record_*` |
| Cortex spans | `aether.cortex.enqueue`, `aether.cortex.flush` | `aether.cortex.index_pass`, `aether.cortex.index_record` |
| Module `muninn_aether_adapters::memory_test` | Named and described as test-only | `muninn_aether_adapters::in_memory`, the zero-infrastructure backend |
| Contract tests | One test file tied to `InMemoryBackend` | Public `muninn_aether_adapters::contract` module any driver runs |
| `muninn-aether-cortex/src/lib.rs` | One 570-line file | Wiring only; `indexer.rs`, `recall.rs`, `ppr.rs` |
| CI | Python only | Rust `fmt`, `clippy`, `test` |

## 5. What Gets Added

| Add | Crate | Purpose |
|-----|-------|---------|
| `RecordStore::create`, `get_many`, `tombstone`, `pending`, `pending_for`, `session_items`, `mark_indexed` | `muninn-aether-core` | Versioned records with indexing state |
| `CreateOutcome`, `PendingRecord` | `muninn-aether-core` | Results of the above |
| `IndexSpec`, `VectorStore::describe`, `IndexSpec::check` | `muninn-aether-core` | Dimension and metric known at the boundary |
| `UserId`, `SessionId` | `muninn-aether-core` | Identity for scopes |
| `DistanceMetric::score`, `FilterExpr::matches` | `muninn-aether-core` | One implementation of scoring and filtering |
| `RecallQuery::validate` | `muninn-aether-core` | Reject unusable queries early |
| `Indexer` (`apply`, `run_pass`, `spawn`) | `muninn-aether-cortex` | The sync engine |
| `index_metadata`, `scope_filter` | `muninn-aether-cortex` | What goes into the index and how scope is filtered there |
| `Candidate`, `recall::vector_search`, `unindexed`, `session`, `graph_search`, `merge`, `rrf`, `in_scopes` | `muninn-aether-cortex` | Recall as composable steps |
| `graph_scores` | `muninn-aether-cortex` | Turn PageRank over entities into scores for memories |
| `jittered` | `muninn-aether-adapters` | Retry jitter |
| `FaultPoint`, `InMemoryBackend::fail_next`, `has_vector` | `muninn-aether-adapters` | Failure-path tests without a real database |
| `contract` module behind feature `contract-suite` | `muninn-aether-adapters` | Same guarantees tested on every backend |
| Builder options `.vector`, `.graph`, `.background_indexing`, `.with_timeout`, `.with_retry` | `muninn-aether-sdk` | Optional stores, opt-in background work, switchable middleware |
| One runnable example per level | `muninn-aether-sdk/examples` | Show each level of use on its own |

## 6. What Stays

- The four crates and their names.
- `MemoryManager` and its four operations: `remember`, `recall`, `update`, `forget`, plus `flush` and `shutdown`.
- `ProjectId` tenant isolation at every store boundary.
- `FilterExpr` and `TemporalFilter`.
- Graph provenance through `source_memory_id` and temporal edges.
- `PersonalizedPageRank` (its output is finally used).
- The `Traced`, `Resilient`, `Timeout`, `Batcher` decorators.
- The `tracing` span model and the `otel` preset.
- Optimistic concurrency on `update`.
- The model-free rule: no embedding, chunking, extraction, or model calls.

## 7. Per-Crate Summary

| Crate | Removed | Changed | Added |
|-------|---------|---------|-------|
| `muninn-aether-core` | `MemoryInbound`, four `RecordStore` methods, `MemoryScope::Global` | `KvStore` to `RecordStore`, `MemoryScope`, `MemoryUpdate`, `RecallQuery` | Versioned record methods, `IndexSpec`, scope ids, `score`, `matches`, validators |
| `muninn-aether-cortex` | `PolicyRouter`, `HotContextCache`, `IngestionBuffer`, `QueryPlanner`, duplicated helpers | Split into modules | `Indexer`, recall steps, `graph_scores`, `index_metadata`, `scope_filter` |
| `muninn-aether-adapters` | Duplicated helpers, empty feature flags | `memory_test` to `in_memory`, idempotent relation upserts, middleware follows new trait | Fault injection, `contract` module, `jittered` |
| `muninn-aether-sdk` | `MemoryManager::new`, `impl MemoryInbound` | Facade rebuilt from public pieces, middleware defaults | Builder options, per-level examples |

## 8. Documentation After the Change

| Page | Action |
|------|--------|
| `docs/index.md` | Keep; update links |
| `docs/architecture/index.md` | Rewrite around the five levels |
| `docs/architecture/sdk-design.md` | Keep; mark implemented |
| `docs/architecture/hardening-design.md` | Keep; mark implemented |
| `docs/architecture/change-overview.md` (this page) | Delete once the work is done; it describes a transition |
| `docs/architecture/connectors.md` | Rewrite: `RecordStore` role, no registry, no circuit breaker, contract module |
| `docs/architecture/rust-migration.md` | Delete |
| `docs/guides/usage.md` | Rewrite: one section and one runnable example per level |
| `docs/guides/memory.md` | Rewrite: roles, scopes with ids, session memory, durability |
| `docs/guides/observability.md` | Update span names |
| `docs/development/guidelines.md` | Replace with short Rust conventions |
| `docs/about/roadmap.md` | Move Phase 5 to completed |
| `README.md` | Rewrite for the Rust SDK |

## 9. Order of Work

1. Rust CI and a clean lint baseline.
2. Shared scoring and filter functions in `muninn-aether-core`.
3. Rename `KvStore` to `RecordStore`.
4. Reusable contract suite and fault injection.
5. Versioned record operations.
6. Idempotent relation upserts.
7. `Indexer`.
8. Recall steps and validation as public pieces.
9. Scopes with identity; session memory record-only.
10. Rebuild `MemoryManager` from the pieces; delete `MemoryInbound`, `QueryPlanner`, `PolicyRouter`, `HotContextCache`, `IngestionBuffer`, and the old `RecordStore` methods.
11. Middleware defaults and jitter.
12. Rename `memory_test` to `in_memory`; remove empty feature flags; remove "gateway" wording from crate docs.
13. Documentation, including deleting `rust-migration.md`.
14. Python removal (after sign-off).

Steps 1 to 7 match tasks 1 to 7 of the earlier gateway-hardening plan.
Steps 8 to 13 replace its tasks 8 to 13, which were written for the gateway shape and have not been revised yet.

## 10. Production Impact

None.
Nothing is published, no production driver exists, and no data is stored anywhere outside tests.
Every public Rust API listed in sections 3 and 4 changes in a breaking way, with no deprecation period, because there are no users to migrate.

## 11. Decisions Still Waiting on the Owner

1. Approval of the pieces-first SDK design as a whole.
2. Sign-off to delete the Python package (section 3.3).
3. Three removals introduced on this page and not yet in an approved design: the empty driver feature flags, the planned `config.rs` / `ConnectorRegistry`, and `docs/architecture/rust-migration.md`.
