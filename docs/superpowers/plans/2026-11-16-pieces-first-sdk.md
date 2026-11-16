# Pieces-First SDK Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task.
> Steps use checkbox (`- [ ]`) syntax for tracking.
> This plan supersedes `docs/superpowers/plans/2026-11-16-gateway-hardening.md`.

**Goal:** Transform Aether from a single-door storage gateway into a pieces-first, model-free Rust memory SDK backed by a durable record store and derived vector/graph indexes.

**Architecture:** Every write, update, and tombstone mutates a versioned record in `RecordStore` (the source of truth).
A composable `Indexer` derives optional vector and graph indexes from pending records, and public `recall::*` functions compose retrieval, scoring, PPR graph fusion, and scope enforcement without hidden background tasks or private facade powers.
`MemoryManager` becomes an optional thin facade built solely from those public pieces.

**Tech Stack:** Rust 2021, tokio, async-trait, serde, serde_json, petgraph, tracing.
No new crate dependencies.

**Specs:**
- `docs/architecture/change-overview.md`
- `docs/architecture/sdk-design.md`
- `docs/architecture/hardening-design.md`

## Global Constraints

- No model calls, embedding, chunking, or extraction anywhere in the workspace.
- No new crate dependencies.
- Every behavioural change starts with a failing test (`AGENTS.md`, Red - Green - Refactor).
- Micro commits: one logical change per commit, conventional prefixes (`feat:`, `fix:`, `refactor:`, `test:`, `docs:`, `chore:`, `ci:`, `style:`), workspace builds and passes tests at every commit.
- Do not add an agent co-author line to commit messages.
- Never use the em dash character in any file; use a plain dash.
- In Markdown files, put each full sentence on its own line.
- Never edit `CHANGELOG.md` or auto-generated files.
- Unit tests never touch live external services.
- Metadata keys starting with `_aether_` are reserved for Aether.
- After Task 1, every commit must pass **the gate**: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, and `cargo test --workspace --all-features`.
- Docs under `docs/` are compiled as MDX; keep generics such as `Arc<T>` and braces such as `{ a, b }` inside backticks or code fences.

## Review Focus

Inputs and failure modes the specs imply that are most likely to bite a user first:

1. Two concurrent `remember` / `create` calls with the same idempotency key: one record is created and both callers get the same `memory_id` (Task 5, Task 11).
2. A retried `create` for the same `MemoryId` without an idempotency key: succeeds as `Created` and stores one record (Task 5).
3. `metadata` that is not a JSON object or contains `_aether_*` keys on input or update: rejected with `InvalidInput` (Task 8).
4. A `query_vector` that is empty or contains `NaN` / infinity: rejected with `InvalidInput` (Task 8).
5. `recall` with `top_k = 0` returns empty without error, and `MemoryManager::recall` in a mode whose optional store is absent returns `InvalidInput` (Task 10, Task 11).

## File Structure

| File | Responsibility | Change |
|------|----------------|--------|
| `.github/workflows/ci.yml` | CI | Add Rust gate job (Task 1), drop Python job (Task 15) |
| `crates/aether-core/src/lib.rs` | Pure contracts, DTOs, scoring, filters | `RecordStore`, `IndexSpec`, `UserId`, `SessionId`, `MemoryScope`, `DistanceMetric::score`, `FilterExpr::matches`, `RecallQuery::validate`; drop `MemoryInbound` and `Global` |
| `crates/aether-core/tests/unit_types.rs` | Core unit tests | Scoring, filters, validation, scope serde |
| `crates/aether-adapters/Cargo.toml` | Crate features | Add `contract-suite` (Task 4); drop empty driver flags (Task 13) |
| `crates/aether-adapters/src/lib.rs` | Module exports | Expose `contract` (Task 4); rename `memory_test` to `in_memory` (Task 13) |
| `crates/aether-adapters/src/in_memory.rs` | Zero-infra reference backend | Versioned records, tombstones, `session_items`, `describe`, idempotent relations, `FaultPoint` (renamed from `memory_test.rs`) |
| `crates/aether-adapters/src/middleware.rs` | Store decorators | Follow trait changes, `record_*` span ops, `jittered` retry backoff |
| `crates/aether-adapters/src/contract.rs` | Reusable contract suite (feature `contract-suite`) | Generic `record_store`, `vector_store`, `graph_store`, concurrency tests |
| `crates/aether-adapters/tests/contract_suite.rs` | Runs contract suite on `InMemoryBackend` | Thin callers + middleware tests |
| `crates/aether-cortex/src/lib.rs` | Module wiring and re-exports only | Shrinks; removes `PolicyRouter`, `HotContextCache`, `IngestionBuffer`, `QueryPlanner`, `enrich_metadata` |
| `crates/aether-cortex/src/indexer.rs` | Sync engine | `Indexer` with optional vector/graph stores, `run_pass`, `apply`, `spawn`, `index_metadata` |
| `crates/aether-cortex/src/ppr.rs` | Graph ranking | `PersonalizedPageRank`, `graph_scores` |
| `crates/aether-cortex/src/recall.rs` | Composable recall pieces | `Candidate`, `RRF_K`, `rrf`, `scope_filter`, `vector_search`, `unindexed`, `session`, `graph_search`, `merge`, `in_scopes` |
| `crates/aether-cortex/tests/unit_indexer.rs` | Indexer unit tests | Fault recovery, tombstones, optional stores, session skip, `spawn` |
| `crates/aether-cortex/tests/unit_recall.rs` | Recall step unit tests | Step composition, real scoring, RRF, graph scoring, scopes |
| `crates/aether-sdk/src/lib.rs` | Optional facade | `MemoryManager` rebuilt from public pieces, builder defaults, no `MemoryInbound` |
| `crates/aether-sdk/tests/hardening.rs` | Guarantee tests G1-G5 | End-to-end guarantee & review-focus suite |
| `crates/aether-sdk/examples/level_*.rs` | Per-level runnable examples | Five examples, one per SDK level (Task 13) |
| `docs/**`, `website/sidebars.ts`, `README.md` | Documentation | Rewritten for pieces-first Rust SDK (Tasks 14-15) |

Shared test fixtures used across test files (define locally per test file):

```rust
fn ns() -> ProjectId { ProjectId::new(format!("t-{}", uuid::Uuid::new_v4())).unwrap() }
fn item(ns: &ProjectId, content: &str, embedding: Option<Vec<f32>>) -> MemoryItem {
    let mut input = MemoryInput::new(ns.clone(), content);
    input.embedding = embedding;
    MemoryItem::from_input(input, 1)
}
```

---

### Task 1: Rust CI and a clean lint baseline

**Files:**
- Modify: `.github/workflows/ci.yml`
- Modify: `crates/**/*.rs` (formatting and clippy fixes only)

**Interfaces:**
- Produces: **the gate** (`cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo test --workspace --all-features`).

- [ ] **Step 1: Format the workspace**

Run: `cargo fmt --all`
Verify: `cargo fmt --all --check` exits `0`.

- [ ] **Step 2: Commit formatting alone**

```bash
git add -A && git commit -m "style: apply rustfmt to the workspace"
```

- [ ] **Step 3: Resolve clippy findings without changing behaviour**

Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
Fix reported warnings idiomatically (without adding `#[allow]` attributes) and re-run until exit `0`.

- [ ] **Step 4: Run tests and commit clippy fixes**

Run: `cargo test --workspace --all-features`
Expected: all existing workspace tests pass.

```bash
git add -A && git commit -m "refactor: resolve clippy warnings across the workspace"
```

- [ ] **Step 5: Add the `rust` job to `.github/workflows/ci.yml`**

Add a job named `rust` beside the existing `test` job on `ubuntu-latest` with:
`actions/checkout@v4`, `dtolnay/rust-toolchain@stable` (`components: rustfmt, clippy`), `Swatinem/rust-cache@v2`, and three named steps running `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, and `cargo test --workspace --all-features`.
Keep the Python `test` job until Task 15.

- [ ] **Step 6: Commit CI change**

```bash
git add .github/workflows/ci.yml && git commit -m "ci: add rust fmt, clippy, and test job"
```

---

### Task 2: Move scoring and filter evaluation into `aether-core`

Two duplicate implementations exist in `aether-cortex/src/lib.rs` (`cosine`, `eval_filter`, `value_matches`) and `aether-adapters/src/memory_test.rs` (`similarity`, `eval_filter`, `value_matches`).

**Files:**
- Modify: `crates/aether-core/src/lib.rs`
- Modify: `crates/aether-adapters/src/memory_test.rs`
- Modify: `crates/aether-cortex/src/lib.rs`
- Test: `crates/aether-core/tests/unit_types.rs`

**Interfaces:**
- Produces in `aether_core`:
  - `DistanceMetric::score(&self, a: &[f32], b: &[f32]) -> f32` (higher is closer; Euclidean returns negative distance `-d`, zero-norm Cosine returns `0.0`).
  - `FilterExpr::matches(&self, metadata: &serde_json::Value) -> bool`.

- [ ] **Step 1: Write failing unit tests in `crates/aether-core/tests/unit_types.rs`**

```rust
#[test]
fn distance_metric_score_is_higher_for_closer_vectors() {
    let q = [1.0, 0.0];
    for m in [DistanceMetric::Cosine, DistanceMetric::Dot, DistanceMetric::Euclidean] {
        assert!(m.score(&q, &[1.0, 0.0]) > m.score(&q, &[0.0, 1.0]), "{m:?}");
    }
    assert_eq!(DistanceMetric::Cosine.score(&q, &[0.0, 0.0]), 0.0);
}

#[test]
fn filter_expr_matches_metadata() {
    let meta = serde_json::json!({"tier": "semantic", "n": 3});
    let eq = FilterExpr::Eq { field: "tier".into(), value: FilterValue::String("semantic".into()) };
    let range = FilterExpr::Range { field: "n".into(), gte: Some(4.0), lte: None };
    assert!(eq.matches(&meta));
    assert!(!range.matches(&meta));
    assert!(FilterExpr::Or(vec![eq.clone(), range.clone()]).matches(&meta));
    assert!(!FilterExpr::And(vec![eq, range]).matches(&meta));
}
```

- [ ] **Step 2: Run and confirm failure**

Run: `cargo test -p aether-core`
Expected: compile error (`score` and `matches` not found).

- [ ] **Step 3: Implement `DistanceMetric::score` and `FilterExpr::matches` in `crates/aether-core/src/lib.rs` and replace all duplicate helpers in `aether-adapters` and `aether-cortex`**

Delete `similarity`, `cosine`, and both copies of `eval_filter` / `value_matches`.

- [ ] **Step 4: Run the gate and commit**

Expected: the gate exits `0`.

```bash
git add -A && git commit -m "refactor(core): single implementation of metric scoring and filter matching"
```

---

### Task 3: Rename `KvStore` to `RecordStore`

Structural rename only; no behaviour changes.

**Files:**
- Modify: `crates/aether-core/src/lib.rs`
- Modify: `crates/aether-cortex/src/lib.rs`, `crates/aether-cortex/tests/unit_cortex.rs`
- Modify: `crates/aether-adapters/src/memory_test.rs`, `crates/aether-adapters/src/middleware.rs`, `crates/aether-adapters/tests/contract_suite.rs`
- Modify: `crates/aether-sdk/src/lib.rs`, `crates/aether-sdk/tests/*.rs`

**Interfaces:**
- Produces: trait `RecordStore` (same methods as `KvStore` today), fields/params named `record_store`, and `Traced` `op` values `record_get`, `record_put`, `record_put_if_absent`, `record_cas_update`, `record_recent`, `record_delete`.

- [ ] **Step 1: Rename `KvStore` -> `RecordStore`, `kv_store` -> `record_store`, and `"kv_*"` -> `"record_*"`**

Update `crates/aether-sdk/tests/tracing_otel.rs` (`"kv_put"` -> `"record_put"`) and rename `contract_kv_store_ttl_idempotency_cas_and_recent` in `crates/aether-adapters/tests/contract_suite.rs` to `contract_record_store_ttl_idempotency_cas_and_recent`.

- [ ] **Step 2: Verify zero leftover occurrences**

Run: `grep -rn "KvStore\|kv_store\|\"kv_" crates/`
Expected: no output.

- [ ] **Step 3: Run the gate and commit**

Expected: the gate exits `0`.

```bash
git add -A && git commit -m "refactor: rename KvStore to RecordStore"
```

---

### Task 4: Reusable contract suite and fault injection

**Files:**
- Create: `crates/aether-adapters/src/contract.rs`
- Modify: `crates/aether-adapters/Cargo.toml`, `crates/aether-adapters/src/lib.rs`, `crates/aether-adapters/src/memory_test.rs`
- Modify: `crates/aether-adapters/tests/contract_suite.rs`

**Interfaces:**
- Produces: feature `contract-suite = []` on `aether-adapters` gating `pub mod contract`.
- Produces in `aether_adapters::contract`:
  - `pub async fn record_store<S: RecordStore>(store: &S)`
  - `pub async fn vector_store<S: VectorStore>(store: &S)`
  - `pub async fn graph_store<S: GraphStore>(store: &S)`
- Produces on `InMemoryBackend`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FaultPoint { VectorUpsert, VectorDelete, GraphUpsert, GraphRetract }

impl InMemoryBackend {
    pub fn fail_next(&self, point: FaultPoint, times: usize);
    pub fn has_vector(&self, ns: &ProjectId, id: MemoryId) -> bool;
}
```

`GraphUpsert` applies to both `upsert_entities` and `upsert_relations`.
Injected faults return `AetherError::Transient { connector: "in_memory", message: "injected fault".into() }`.

- [ ] **Step 1: Write the failing fault-injection test in `crates/aether-adapters/tests/contract_suite.rs`**

```rust
#[tokio::test]
async fn in_memory_backend_injects_transient_faults_then_recovers() {
    let b = InMemoryBackend::new();
    let p = ProjectId::new(format!("t-{}", uuid::Uuid::new_v4())).unwrap();
    b.initialize(&p, 2, DistanceMetric::Cosine).await.unwrap();
    let rec = VectorRecord { id: MemoryId::new(), vector: vec![1.0, 0.0], metadata: serde_json::json!({}) };
    b.fail_next(FaultPoint::VectorUpsert, 1);
    let err = b.upsert_batch(&p, &[rec.clone()]).await.unwrap_err();
    assert!(err.is_retryable());
    assert!(!b.has_vector(&p, rec.id));
    b.upsert_batch(&p, &[rec.clone()]).await.unwrap();
    assert!(b.has_vector(&p, rec.id));
}
```

- [ ] **Step 2: Run and confirm failure**

Run: `cargo test -p aether-adapters --test contract_suite in_memory_backend_injects`
Expected: compile error (`FaultPoint` not found).

- [ ] **Step 3: Implement `FaultPoint`, `fail_next`, and `has_vector` in `crates/aether-adapters/src/memory_test.rs`**

Check and decrement fault counts in `Mutex<HashMap<FaultPoint, usize>>` at the top of each affected method.

- [ ] **Step 4: Run test, run the gate, and commit**

```bash
git add crates/aether-adapters/src/memory_test.rs crates/aether-adapters/tests/contract_suite.rs
git commit -m "test(adapters): add fault injection to InMemoryBackend"
```

- [ ] **Step 5: Extract generic contract tests into `crates/aether-adapters/src/contract.rs`**

Enable `contract-suite = []` under `[features]` and `aether-adapters = { path = ".", features = ["contract-suite"] }` under `[dev-dependencies]` in `crates/aether-adapters/Cargo.toml`.
Gate `#[cfg(feature = "contract-suite")] pub mod contract;` in `crates/aether-adapters/src/lib.rs`.
Move the five backend contract test bodies from `tests/contract_suite.rs` into `contract::record_store`, `contract::vector_store`, and `contract::graph_store` using fresh `ProjectId`s per function so shared real backends can run them safely.
Keep thin `#[tokio::test]` callers plus `contract_resilient_retries_transient_only_and_batcher_chunks` in `tests/contract_suite.rs`.

- [ ] **Step 6: Run the gate and commit**

Expected: the gate exits `0`.

```bash
git add -A && git commit -m "refactor(adapters): make the contract suite reusable across backends"
```

---

### Task 5: `RecordStore` versioned-record operations

Additive task.
Old methods (`put`, `put_if_absent`, `recent`, `delete`) remain until Task 11 removes them.

**Files:**
- Modify: `crates/aether-core/src/lib.rs` (types, `RecordStore` methods, `Arc<T>` forwarding impl)
- Modify: `crates/aether-adapters/src/memory_test.rs`
- Modify: `crates/aether-adapters/src/middleware.rs` (`Resilient`, `Timeout`, `Traced`)
- Modify: `crates/aether-adapters/src/contract.rs`
- Modify: `crates/aether-adapters/tests/contract_suite.rs`

**Interfaces:**
- Produces in `aether_core`:

```rust
#[derive(Debug, Clone, PartialEq)]
pub enum CreateOutcome { Created, Duplicate(MemoryReceipt) }

#[derive(Debug, Clone, PartialEq)]
pub struct PendingRecord { pub item: MemoryItem, pub tombstone: bool }

// Added to RecordStore
async fn create(&self, ns: &ProjectId, item: &MemoryItem, ttl: Option<Duration>) -> Result<CreateOutcome>;
async fn get_many(&self, ns: &ProjectId, ids: &[MemoryId]) -> Result<Vec<MemoryItem>>;
async fn tombstone(&self, ns: &ProjectId, id: MemoryId) -> Result<Option<MemoryItem>>;
async fn pending(&self, limit: usize) -> Result<Vec<PendingRecord>>;
async fn pending_for(&self, ns: &ProjectId, limit: usize) -> Result<Vec<MemoryItem>>;
async fn mark_indexed(&self, ns: &ProjectId, id: MemoryId, version: u64) -> Result<()>;
```

Contract semantics:
- `create` stores the item as pending and, if `item.idempotency_key` is `Some`, binds the key atomically under one write lock.
- `create` with a key already bound to a live record returns `Duplicate` with that record's `memory_id`, current `version`, and `state` (`Persisted` if indexed, `Buffered` otherwise).
- `create` with an existing `MemoryId` (retried create without key) returns `Created` and keeps one record.
- `get` and `get_many` never return tombstoned or expired records.
- `cas_update` marks the updated record pending.
- `tombstone` bumps `version` by 1, marks the record tombstoned/pending, immediately releases its idempotency key, and returns the updated `MemoryItem` (or `None` if no live record exists).
- `pending` returns live-pending and tombstoned records across all projects ordered oldest-change-first (`seq`), excluding expired records.
- `pending_for` returns live-pending records for one project only, excluding tombstones and expired records.
- `mark_indexed` is a no-op unless `version` equals the record's current `version`; when matching a tombstone, it removes the record.

- [ ] **Step 1: Add contract cases and concurrency test to `crates/aether-adapters/src/contract.rs` and `tests/contract_suite.rs`**

Add assertions to `contract::record_store` covering:
- `create` -> `pending_for` / `pending` -> stale `mark_indexed` (no-op) -> matching `mark_indexed` -> `get`.
- Retried `create` for the same `MemoryId` -> `Created`, `get_many` returns 1 item (Review Focus 2).
- Atomic idempotency key binding (`Duplicate(Buffered)`), releasing key on `tombstone`.
- `cas_update` re-entering `pending_for`.
- `tombstone` bumping `version`, hiding from `get`/`pending_for`, keeping in `pending` until `mark_indexed` removes it, returning `None` on missing id.
- Expired records invisible to `get` and `pending`.

Add `pub async fn record_store_concurrent_idempotency<S: RecordStore + 'static>(store: Arc<S>)` spawning 16 tasks with the same `idempotency_key` and asserting `Created` count is `1` and all returned `memory_id`s match (Review Focus 1); invoke it from `tests/contract_suite.rs` with `#[tokio::test(flavor = "multi_thread")]`.

- [ ] **Step 2: Run and confirm failure**

Run: `cargo test -p aether-adapters`
Expected: compile error (`create`, `CreateOutcome`, `PendingRecord` not found).

- [ ] **Step 3: Add types and trait methods to `crates/aether-core/src/lib.rs` (including `Arc<T>` impl)**

- [ ] **Step 4: Implement versioned records in `InMemoryBackend` (`crates/aether-adapters/src/memory_test.rs`)**

Extend `KvEntry` (rename `RecordEntry`) with `indexed_version: u64` (`0` when unindexed) and `tombstone: bool`.
Pending means `indexed_version != item.version`.
Update `seq` from `next_seq` on every state change to order `pending`.

- [ ] **Step 5: Implement delegations in `Resilient`, `Timeout`, and `Traced` (`crates/aether-adapters/src/middleware.rs`)**

`Traced` op names: `record_create`, `record_get_many`, `record_tombstone`, `record_pending`, `record_pending_for`, `record_mark_indexed` (`record_pending` omits `project_id`).

- [ ] **Step 6: Run the gate and commit**

Expected: the gate exits `0`.

```bash
git add -A && git commit -m "feat(core,adapters): add versioned record operations to RecordStore"
```

---

### Task 6: Idempotent relation upserts in `GraphStore`

**Files:**
- Modify: `crates/aether-core/src/lib.rs` (doc comment on `GraphStore::upsert_relations`)
- Modify: `crates/aether-adapters/src/memory_test.rs`
- Modify: `crates/aether-adapters/src/contract.rs`

**Interfaces:**
- Produces: contract rule that an edge is keyed by `(source_memory_id, source, target, relation_type)`; `upsert_relations` replaces an existing edge with that key (including retracted edges) instead of appending a duplicate.

- [ ] **Step 1: Add the failing test case to `contract::graph_store` in `crates/aether-adapters/src/contract.rs`**

Upsert the same relation twice for one `MemoryId`, assert `neighborhood` returns `1` edge; call `retract_memory_facts`, assert `0` active edges; call `upsert_relations` again with the same relation, assert `1` active edge and `1` total edge (`active_only: false`).

- [ ] **Step 2: Run and confirm failure**

Run: `cargo test -p aether-adapters --test contract_suite`
Expected: FAIL (`edges.len()` is `2`, expected `1`).

- [ ] **Step 3: Update `InMemoryBackend::upsert_relations` to replace matching edge weights and document the rule on `GraphStore::upsert_relations`**

- [ ] **Step 4: Run the gate and commit**

```bash
git add -A && git commit -m "fix(adapters): make relation upserts idempotent per source memory"
```

---

### Task 7: `Indexer` with optional stores and opt-in `spawn`

**Files:**
- Create: `crates/aether-cortex/src/indexer.rs`
- Modify: `crates/aether-cortex/src/lib.rs` (`pub mod indexer; pub use indexer::{index_metadata, Indexer, PassReport};`)
- Create: `crates/aether-cortex/tests/unit_indexer.rs`

**Interfaces:**
- Consumes: `RecordStore::{pending, mark_indexed}`, `PendingRecord`, `VectorStore::{upsert_batch, delete}`, `GraphStore::{upsert_entities, upsert_relations, retract_memory_facts}`.
- Produces in `aether_cortex::indexer`:

```rust
/// Metadata written to the vector index: caller metadata object plus `_aether_tier`.
pub fn index_metadata(item: &MemoryItem) -> serde_json::Value;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PassReport { pub indexed: usize, pub failed: usize }

pub struct Indexer { /* record, vector: Option<Arc<dyn VectorStore>>, graph: Option<Arc<dyn GraphStore>>, notify: Arc<Notify> */ }

impl Indexer {
    pub fn new(record: Arc<dyn RecordStore>) -> Self;
    pub fn with_vector(self, vector: Arc<dyn VectorStore>) -> Self;
    pub fn with_graph(self, graph: Arc<dyn GraphStore>) -> Self;
    pub fn notify(&self);
    pub async fn apply(&self, record: &PendingRecord) -> Result<()>;
    pub async fn run_pass(&self) -> Result<PassReport>;
    pub fn spawn(self: &Arc<Self>, interval: Duration) -> tokio::task::JoinHandle<()>;
}
```

Behaviour:
- `apply` (only executes vector calls when `self.vector.is_some()`, graph calls when `self.graph.is_some()`):
  1. Tombstone: delete from vector (if configured), `retract_memory_facts` on graph (if configured), then `mark_indexed`.
  2. Live: if `embedding` is `Some`, `upsert_batch` one `VectorRecord` with `index_metadata(item)`; otherwise `vector.delete([id])` (if vector configured). Then `retract_memory_facts`, and when non-empty `upsert_entities` / `upsert_relations` (if graph configured). Finally `mark_indexed(ns, id, item.version)`.
- `content` is never written into `index_metadata`.
- `run_pass` pulls `pending(256)` in a loop, skips ids that already failed during this pass, and stops when a batch yields nothing new (`// ponytail: a pass stalls if 256+ records fail permanently; add a cursor to pending() if that ever happens.`). Returns `Err` with the last failure if `failed > 0`, else `Ok(PassReport)`.
- `spawn` runs a `tokio::spawn` loop holding `Arc::downgrade(self)` (exits when `Indexer` is dropped), waking on `notify.notified()` or `tokio::time::sleep(interval)`, calling `run_pass()` and logging errors with `tracing::warn!`.
- Spans: `aether.cortex.index_pass` (`indexed`, `failed`) and `aether.cortex.index_record` (`project_id`, `memory_id`, `tombstone`).

- [ ] **Step 1: Write failing tests in `crates/aether-cortex/tests/unit_indexer.rs`**

Cover:
1. `failed_index_write_keeps_the_record_pending_and_indexes_the_rest` (fault-inject `VectorUpsert`, assert partial progress and recovery on second `run_pass`).
2. `tombstone_removes_vector_and_facts_then_the_record` (fault-inject `VectorDelete`, assert pending retry and cleanup).
3. `item_without_embedding_has_its_stale_vector_removed`.
4. `indexer_with_no_vector_or_graph_store_marks_records_indexed`.
5. `index_metadata_adds_tier_and_omits_content`.
6. `spawn_indexes_when_notified` (using `Indexer::spawn` + `notify()`).

- [ ] **Step 2: Run and confirm failure**

Run: `cargo test -p aether-cortex --test unit_indexer`
Expected: compile error (`Indexer` not found).

- [ ] **Step 3: Implement `crates/aether-cortex/src/indexer.rs` and export from `lib.rs`**

- [ ] **Step 4: Run the gate and commit**

```bash
git add -A && git commit -m "feat(cortex): add Indexer with optional vector/graph stores and opt-in spawn"
```

---

### Task 8: Boundary validation and `IndexSpec`

**Files:**
- Modify: `crates/aether-core/src/lib.rs`
- Modify: `crates/aether-adapters/src/memory_test.rs`, `crates/aether-adapters/src/middleware.rs`, `crates/aether-adapters/src/contract.rs`
- Modify: `crates/aether-cortex/src/lib.rs`, `crates/aether-sdk/src/lib.rs` (update `MemoryUpdate` constructors in existing files/tests as needed to keep workspace green)
- Test: `crates/aether-core/tests/unit_types.rs`

**Interfaces:**
- Produces in `aether_core`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexSpec { pub dimensions: usize, pub metric: DistanceMetric }

impl IndexSpec {
    pub fn check(&self, vector: &[f32]) -> Result<()>;
}

// Added to VectorStore
async fn describe(&self, ns: &ProjectId) -> Result<Option<IndexSpec>>;

// MemoryUpdate gains
pub entities: Option<Vec<Entity>>,
pub relations: Option<Vec<Relation>>,

impl MemoryUpdate {
    pub fn validate(&self) -> Result<()>;
}

impl RecallQuery {
    pub fn validate(&self) -> Result<()>;
}
```

Validation rules (all returning `AetherError::InvalidInput`):
- `MemoryInput::validate`: `metadata` must be `Value::Object`; no metadata key starts with `_aether_` (Review Focus 3).
- `MemoryUpdate::validate`: if `content` is `Some`, it must be non-empty trimmed; if `embedding` is `Some`, it must be non-empty and all finite; if `metadata` is `Some`, it must be `Value::Object` with no `_aether_*` keys; validate any `entities`/`relations` non-empty names/types.
- `IndexSpec::check(vector)`: `vector` must be non-empty, finite, and have `vector.len() == self.dimensions`.
- `RecallQuery::validate`: `Vector` or `Hybrid` without `query_vector` -> `InvalidInput`; `Graph` with empty `seed_entities` -> `InvalidInput`; `query_vector` empty or containing non-finite values -> `InvalidInput` (Review Focus 4).

- [ ] **Step 1: Write failing tests in `crates/aether-core/tests/unit_types.rs` and `crates/aether-adapters/src/contract.rs`**

In `unit_types.rs`:
- `memory_input_rejects_non_object_and_reserved_metadata` (Review Focus 3).
- `memory_update_rejects_invalid_metadata_and_non_finite_embedding`.
- `index_spec_check_validates_dimensions_and_finiteness`.
- `recall_query_validation` (Hybrid without vector, Graph without seeds, NaN query vector - Review Focus 4).

In `contract::vector_store`:
- `describe` returns `None` before `initialize`, and `Some(IndexSpec { dimensions: 3, metric: DistanceMetric::Cosine })` after `initialize(ns, 3, Cosine)`.

- [ ] **Step 2: Run and confirm failure**

Run: `cargo test -p aether-core -p aether-adapters`
Expected: compile errors (`IndexSpec`, `describe`, `RecallQuery::validate` not found).

- [ ] **Step 3: Implement `IndexSpec`, `VectorStore::describe`, `MemoryUpdate` fields + `validate()`, `MemoryInput::validate` metadata checks, and `RecallQuery::validate()`**

Implement `describe` across `InMemoryBackend`, `Arc<T>`, `Resilient`, `Timeout`, `Batcher`, and `Traced` (`op = "describe"`).
Update existing `MemoryUpdate { .. }` initializers in `crates/aether-sdk/tests/` and `crates/aether-sdk/examples/otel_demo.rs` to include `entities: None, relations: None`.

- [ ] **Step 4: Run the gate and commit**

```bash
git add -A && git commit -m "feat(core,adapters): add IndexSpec, VectorStore::describe, and boundary validators"
```

---

### Task 9: Scopes with identity and record-only session memory

**Files:**
- Modify: `crates/aether-core/src/lib.rs`
- Modify: `crates/aether-adapters/src/memory_test.rs`, `crates/aether-adapters/src/middleware.rs`, `crates/aether-adapters/src/contract.rs`
- Modify: `crates/aether-cortex/src/indexer.rs`
- Modify: `crates/aether-sdk/src/lib.rs` (compile-compatibility for `MemoryScope` match)
- Test: `crates/aether-core/tests/unit_types.rs`, `crates/aether-cortex/tests/unit_indexer.rs`

**Interfaces:**
- Produces in `aether_core`:

```rust
pub struct UserId(String);    // validated non-empty, as_str(), Display, serde transparent
pub struct SessionId(String); // validated non-empty, as_str(), Display, serde transparent

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum MemoryScope { #[default] Project, User(UserId), Session(SessionId) }

impl MemoryScope {
    /// "project" or "user:<id>". None for sessions, which are never indexed.
    pub fn index_key(&self) -> Option<String>;
}

// RecallQuery gains (default vec![MemoryScope::Project]); RecallQuery::validate() rejects empty scopes
pub scopes: Vec<MemoryScope>,

// Added to RecordStore
async fn session_items(&self, ns: &ProjectId, session: &SessionId, limit: usize) -> Result<Vec<MemoryItem>>;
```

Removes `MemoryScope::Global` (and `Copy` on `MemoryScope`).
Updates `index_metadata` in `crates/aether-cortex/src/indexer.rs` to write `_aether_scope` (`index_key()`) when `Some`, and updates `Indexer::apply` so live `MemoryScope::Session(_)` items skip vector/graph writes and only call `mark_indexed`.

- [ ] **Step 1: Write failing tests**

In `crates/aether-core/tests/unit_types.rs`:
- `memory_scope_serde_and_index_key`: `User("u1")` serializes as `{"kind": "user", "id": "u1"}`, `Project` as `{"kind": "project"}`, `index_key()` returns `Some("user:u1")` / `Some("project")` / `None`, `UserId::new(" ")` fails, and `RecallQuery` with `scopes: vec![]` fails `validate()`.

In `crates/aether-adapters/src/contract.rs` (`contract::record_store`):
- `session_items` returns only that session's live, unexpired items (not another session's, not `Project` items, not expired or tombstoned ones).

In `crates/aether-cortex/tests/unit_indexer.rs`:
- `session_items_are_marked_indexed_without_writing_to_vector_or_graph`.
- Update `index_metadata` test to assert `_aether_scope == "project"` for project scope.

- [ ] **Step 2: Run and confirm failure**

Run: `cargo test --workspace`
Expected: compile error (`UserId`, `SessionId`, `scopes`, `session_items` not found).

- [ ] **Step 3: Implement `UserId`, `SessionId`, `MemoryScope`, `RecallQuery.scopes`, `RecordStore::session_items` (backend, `Arc`, `Resilient`, `Timeout`, `Traced` with `op = "record_session_items"`), and `Indexer` session handling**

- [ ] **Step 4: Run the gate and commit**

```bash
git add -A && git commit -m "feat(core,cortex,adapters): add scope identities and record-only session memory"
```

---

### Task 10: Public recall steps, PPR graph scoring, and RRF fusion

**Files:**
- Create: `crates/aether-cortex/src/ppr.rs`
- Create: `crates/aether-cortex/src/recall.rs`
- Modify: `crates/aether-cortex/src/lib.rs` (export `ppr` and `recall` modules and types)
- Create: `crates/aether-cortex/tests/unit_recall.rs`

**Interfaces:**
- Produces in `aether_cortex::ppr`:
  - `PersonalizedPageRank` (moved from `lib.rs`).
  - `pub fn graph_scores(subgraph: &Subgraph, entity_ranks: &[(EntityId, f32)]) -> HashMap<MemoryId, f32>` - for each edge in `subgraph`, adds `rank[edge.source] + rank[edge.target]` to `scores[edge.source_memory_id]`.
- Produces in `aether_cortex::recall`:

```rust
pub const RRF_K: f32 = 60.0;

#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub item: MemoryItem,
    pub score: f32,
}

pub fn scope_filter(scopes: &[MemoryScope]) -> Option<FilterExpr>;

pub async fn vector_search(
    vectors: &dyn VectorStore,
    records: &dyn RecordStore,
    ns: &ProjectId,
    query_vector: &[f32],
    top_k: usize,
    filter: Option<&FilterExpr>,
) -> Result<Vec<Candidate>>;

pub async fn unindexed(
    records: &dyn RecordStore,
    ns: &ProjectId,
    query_vector: &[f32],
    metric: DistanceMetric,
    filter: Option<&FilterExpr>,
) -> Result<Vec<Candidate>>;

pub async fn session(
    records: &dyn RecordStore,
    ns: &ProjectId,
    session_id: &SessionId,
    query_vector: &[f32],
    metric: DistanceMetric,
    filter: Option<&FilterExpr>,
) -> Result<Vec<Candidate>>;

pub async fn graph_search(
    graph: &dyn GraphStore,
    records: &dyn RecordStore,
    ns: &ProjectId,
    seeds: &[EntityId],
    hops: u8,
    per_node_limit: usize,
    temporal: &TemporalFilter,
    ppr: &PersonalizedPageRank,
) -> Result<(Vec<Candidate>, Subgraph)>;

pub fn merge(lists: Vec<Vec<Candidate>>) -> Vec<Candidate>;
pub fn rrf(lists: &[Vec<Candidate>], k: f32) -> Vec<Candidate>;
pub fn in_scopes(candidates: Vec<Candidate>, scopes: &[MemoryScope]) -> Vec<Candidate>;
```

Exact semantics:
- `vector_search`: searches `vectors`, resolves hit IDs via `records.get_many`, drops unresolved IDs (deleted/expired), returns `Candidate { item, score }` sorted descending by `score`, then `item.id` ascending. Emits span `aether.cortex.vector_search`.
- `unindexed`: reads `records.pending_for(ns, 256)`, filters with `filter.matches(&index_metadata(item))` when `filter` is `Some`, keeps items with an embedding of `query_vector.len()`, scores with `metric.score(query_vector, emb)`, sorts descending.
- `session`: reads `records.session_items(ns, session_id, 256)`, applies the same `filter` and embedding-length/metric scoring as `unindexed`.
- `graph_search`: if `seeds.is_empty()` returns `(Vec::new(), Subgraph::default())`; else fetches `neighborhood`, runs `ppr.rank(&subgraph, seeds)`, computes `graph_scores`, resolves memory IDs via `records.get_many`, drops unresolved, sorts descending by score then `item.id`. Emits span `aether.cortex.graph_search`.
- `merge`: combines same-metric lists, deduplicating by `item.id` keeping the entry with the highest `item.version` (tie-break highest `score`), sorted descending by `score` then `item.id`.
- `rrf`: computes `sum over lists of 1.0 / (k + rank)` (`rank` 1-based within each list), keeping newest `MemoryItem` per `item.id`, sorted descending by RRF score then `item.id`.
- `in_scopes`: keeps candidates where `scopes.contains(&c.item.scope)`.
- `scope_filter`: collects unique `scope.index_key()` values; returns `None` if empty, otherwise `Some(FilterExpr::In { field: "_aether_scope".into(), values })`.

- [ ] **Step 1: Move `PersonalizedPageRank` into `crates/aether-cortex/src/ppr.rs` as a pure refactor**

```bash
git add -A && git commit -m "refactor(cortex): move PersonalizedPageRank into ppr module"
```

- [ ] **Step 2: Write failing tests in `crates/aether-cortex/tests/unit_recall.rs`**

Cover:
1. `vector_search_resolves_through_records_and_drops_tombstoned_hits`.
2. `unindexed_scores_with_index_metric_and_merge_keeps_newest_version`.
3. `session_step_returns_only_live_matching_session_items`.
4. `graph_search_and_graph_scores_rank_memories_by_edge_ppr`.
5. `rrf_fuses_multiple_rankings_deterministically`.
6. `scope_filter_and_in_scopes_enforce_user_isolation`.

- [ ] **Step 3: Run and confirm failure**

Run: `cargo test -p aether-cortex --test unit_recall`
Expected: compile error (`recall` module / `graph_scores` not found).

- [ ] **Step 4: Implement `graph_scores` in `ppr.rs` and `crates/aether-cortex/src/recall.rs`**

- [ ] **Step 5: Run the gate and commit**

```bash
git add -A && git commit -m "feat(cortex): add composable recall steps, graph scoring, and RRF fusion"
```

---

### Task 11: Rebuild `MemoryManager` from public pieces and delete superseded abstractions

This task switches `MemoryManager` to the new architecture and deletes `MemoryInbound`, `QueryPlanner`, `PolicyRouter`, `HotContextCache`, `IngestionBuffer`, `enrich_metadata`, and the old `RecordStore` methods.

**Files:**
- Modify: `crates/aether-sdk/src/lib.rs`
- Modify: `crates/aether-core/src/lib.rs` (remove `MemoryInbound` and old `RecordStore::{put, put_if_absent, recent, delete}`)
- Modify: `crates/aether-cortex/src/lib.rs` (remove `PolicyRouter`, `PolicyDecision`, `HotContextCache`, `IngestionBuffer`, `QueryPlanner`, `enrich_metadata`)
- Modify: `crates/aether-adapters/src/{memory_test,middleware,contract}.rs`, `crates/aether-adapters/tests/contract_suite.rs`
- Create: `crates/aether-sdk/tests/hardening.rs`
- Modify: `crates/aether-sdk/tests/{integration_memory_manager,e2e_brain_and_scale,tracing_otel}.rs`, `crates/aether-sdk/examples/otel_demo.rs`, `crates/aether-cortex/tests/unit_cortex.rs`

**Interfaces:**
- Produces in `aether_sdk`:

```rust
pub struct MemoryManagerBuilder { /* ... */ }
impl MemoryManagerBuilder {
    pub fn vector(self, vector_store: Arc<dyn VectorStore>) -> Self;
    pub fn graph(self, graph_store: Arc<dyn GraphStore>) -> Self;
    pub fn background_indexing(self, interval: Option<Duration>) -> Self; // default None
    pub fn with_connector_tracing(self, enabled: bool) -> Self;           // default true
    pub fn with_session_ttl(self, ttl: Option<Duration>) -> Self;         // default 1h
    pub fn build(self) -> MemoryManager;
}

impl MemoryManager {
    pub fn builder(record_store: Arc<dyn RecordStore>) -> MemoryManagerBuilder;
    pub async fn remember(&self, input: MemoryInput) -> Result<MemoryReceipt>;
    pub async fn recall(&self, query: RecallQuery) -> Result<RecallResult>;
    pub async fn update(&self, project_id: &ProjectId, memory_id: MemoryId, patch: MemoryUpdate) -> Result<MemoryReceipt>;
    pub async fn forget(&self, project_id: &ProjectId, memory_id: MemoryId) -> Result<()>;
    pub async fn flush(&self) -> Result<()>;
    pub async fn shutdown(&self) -> Result<()>;
}
```

`MemoryManager` is composed **only** from public pieces in `aether_core`, `aether_cortex`, and `aether_adapters`:
- `remember`: check not shut down -> `input.validate()` -> if `vector_store` is present and `input.embedding` is `Some`, check `spec(&input.project_id)?.check(emb)` -> `record_store.create` (with `session_ttl` for `Session(_)`) -> on `Duplicate(r)` return `r` -> on `Buffered`, if `background_interval` is `Some` lazily start `Indexer::spawn` once and call `indexer.notify()`, return `ReceiptState::Buffered` -> on `WaitFlush`, call `indexer.apply(&PendingRecord { item, tombstone: false })` and return `ReceiptState::Persisted`.
- `update`: `patch.validate()` -> load `current` via `record_store.get` (`NotFound` if missing) -> check `expected_version` -> reject `patch.content.is_some() && patch.embedding.is_none() && current.embedding.is_some()` with `InvalidInput` -> apply patch (`content`, `embedding`, `metadata`, `entities`, `relations`), bump `version`, `updated_at` -> if `vector_store` is present and `current.embedding` is `Some`, check `spec(project_id)?.check(emb)` -> `record_store.cas_update` -> `indexer.apply` -> return `Persisted` receipt.
- `forget`: `record_store.tombstone` -> if `None` return `Ok(())` -> else `indexer.apply(&PendingRecord { item, tombstone: true })`.
- `recall`: `query.validate()` -> check required optional stores (`Vector`/`Hybrid` needs `vector_store`; `Graph`/`Hybrid` needs `graph_store`, else `InvalidInput`) -> if `query.top_k == 0` return `RecallResult::default()` -> compose `recall::scope_filter`, `recall::vector_search`, `recall::unindexed`, `recall::session`, `recall::merge`, `recall::graph_search` (with seeds = `query.seed_entities` + entities/endpoints from vector hits in `Hybrid`), `recall::rrf` (`RRF_K`), `recall::in_scopes`, truncate to `query.top_k`.
- `flush`: `indexer.run_pass().await.map(|_| ())`.
- `shutdown`: mark shut down (later `remember` returns `InvalidInput`), abort background handle if spawned, `indexer.run_pass()`, `close()` all configured stores.

- [ ] **Step 1: Write failing guarantee tests in `crates/aether-sdk/tests/hardening.rs`**

Cover G1-G5 and Review Focus 1 & 5:
- `failed_flush_loses_nothing` (G1)
- `buffered_writes_survive_a_rebuilt_manager` (G1)
- `buffered_writes_are_not_indexed_without_opt_in_and_are_with_opt_in` (S4 / G1)
- `forgetting_an_unflushed_memory_does_not_resurrect_it` (G2)
- `failed_forget_hides_the_memory_and_is_retried` (G2)
- `unflushed_and_flushed_copies_score_the_same` (G3)
- `an_unrelated_unflushed_write_does_not_outrank_a_real_match` (G3)
- `eventual_consistency_skips_unflushed_writes`
- `top_k_zero_returns_empty` (Review Focus 5)
- `hybrid_surfaces_a_graph_connected_memory_the_vector_search_missed`
- `one_user_cannot_recall_another_users_memory` (G4)
- `session_memory_is_recalled_while_live_never_indexed_and_gone_after_ttl` (G4)
- `wrong_dimension_and_uninitialized_project_are_rejected` (G5)
- `update_content_requires_new_embedding_and_replaces_facts`
- `concurrent_remembers_with_one_key_create_one_memory` (Review Focus 1)
- `record_only_manager_works_and_errors_on_vector_or_graph_recall` (Review Focus 5)

- [ ] **Step 2: Run and confirm failure**

Run: `cargo test -p aether-sdk --test hardening`
Expected: compile error (`MemoryManager::builder(b)` signature changed).

- [ ] **Step 3: Rebuild `MemoryManager` & `MemoryManagerBuilder` in `crates/aether-sdk/src/lib.rs`, and update `integration_memory_manager.rs`, `e2e_brain_and_scale.rs`, `tracing_otel.rs`, and `examples/otel_demo.rs`**

In `tracing_otel.rs`, assert `aether.cortex.index_record` under `aether.remember`, `aether.cortex.vector_search` under `aether.recall`, and `record_create` for the `Traced` op.

- [ ] **Step 4: Run the gate and commit the facade switch**

```bash
git add -A && git commit -m "feat(sdk): rebuild MemoryManager from composable pieces and Indexer"
```

- [ ] **Step 5: Delete `MemoryInbound`, `QueryPlanner`, `PolicyRouter`, `PolicyDecision`, `HotContextCache`, `IngestionBuffer`, `enrich_metadata`, and their obsolete tests in `unit_cortex.rs`**

Run: `grep -rn "MemoryInbound\|QueryPlanner\|IngestionBuffer\|HotContextCache\|PolicyRouter\|PolicyDecision\|enrich_metadata" crates/`
Expected: no output.
Run the gate, then commit:

```bash
git add -A && git commit -m "refactor(core,cortex): remove MemoryInbound, QueryPlanner, PolicyRouter, HotContextCache, and IngestionBuffer"
```

- [ ] **Step 6: Delete `RecordStore::{put, put_if_absent, recent, delete}` across `aether-core`, `aether-adapters`, and the old contract tests (retaining TTL, isolation, and CAS coverage via `create`)**

Run the gate, then commit:

```bash
git add -A && git commit -m "refactor(core,adapters): remove superseded RecordStore methods"
```

---

### Task 12: Middleware defaults and retry jitter

**Files:**
- Modify: `crates/aether-adapters/src/middleware.rs`
- Modify: `crates/aether-sdk/src/lib.rs`
- Modify: `crates/aether-adapters/tests/contract_suite.rs`, `crates/aether-sdk/tests/hardening.rs`

**Interfaces:**
- Produces in `aether_adapters::middleware`: `pub fn jittered(backoff: Duration, random: u64) -> Duration` returning a value in `[0, backoff]` (full jitter); `retry_transient` uses `jittered` with `std::collections::hash_map::RandomState`.
- Produces on `MemoryManagerBuilder`:
  - `pub fn with_timeout(self, timeout: Option<Duration>) -> Self` (default `Some(Duration::from_secs(10))`).
  - `pub fn with_retry(self, policy: Option<RetryPolicy>) -> Self` (default `Some(RetryPolicy::default())`).
- `build()` wraps each configured store innermost-first: `Batcher` (vector store only, always), `Timeout` if `Some`, `Resilient` if `Some`, `Traced` if `connector_tracing`.

- [ ] **Step 1: Write failing tests**

In `crates/aether-adapters/tests/contract_suite.rs`:
`jittered_backoff_stays_within_bounds`.
In `crates/aether-sdk/tests/hardening.rs`:
`default_manager_retries_transient_connector_failures` (fails `VectorUpsert` twice, `remember(WaitFlush)` succeeds via default `Resilient`).

- [ ] **Step 2: Run and confirm failure**

Run: `cargo test --workspace`
Expected: `jittered` not found / transient error surfaces before retry middleware is wired.

- [ ] **Step 3: Implement `jittered` and builder defaults; set `.with_retry(None)` in `hardening.rs` single-fault test helper**

- [ ] **Step 4: Run the gate and commit**

```bash
git add -A && git commit -m "feat(adapters,sdk): add full jitter to retries and apply Timeout/Resilient defaults"
```

---

### Task 13: Module cleanup, terminology, and per-level examples

**Files:**
- Rename: `crates/aether-adapters/src/memory_test.rs` -> `crates/aether-adapters/src/in_memory.rs`
- Modify: `crates/aether-adapters/src/lib.rs`, `crates/aether-adapters/Cargo.toml`
- Modify: `crates/*/src/**/*.rs` (doc comments only: remove `"gateway"` wording)
- Create: `crates/aether-sdk/examples/level_1_connector.rs`, `level_2_functions.rs`, `level_3_indexer.rs`, `level_4_recall_steps.rs`, `level_5_facade.rs`

**Interfaces:**
- Produces: `aether_adapters::in_memory::InMemoryBackend` (re-exported as `aether_adapters::InMemoryBackend`).
- Removes empty feature flags `turbopuffer`, `qdrant`, `neo4j`, `chroma`, `pgvector` from `crates/aether-adapters/Cargo.toml`.

- [ ] **Step 1: Rename `memory_test.rs` to `in_memory.rs` and drop empty driver flags in `crates/aether-adapters/Cargo.toml`**

- [ ] **Step 2: Update crate/module doc comments to remove `"gateway"` and `"L1/L2/L3"` wording**

Run: `grep -rni "gateway" crates/`
Expected: no matches in doc comments or descriptions.

- [ ] **Step 3: Add five runnable examples in `crates/aether-sdk/examples/` (one per level of `sdk-design.md` §4.1)**

Run each with `cargo run -p aether-sdk --example <name>` and verify exit `0`.

- [ ] **Step 4: Run the gate and commit**

```bash
git add -A && git commit -m "refactor(adapters,sdk): rename in_memory module, drop empty flags, add per-level examples"
```

---

### Task 14: Documentation overhaul

**Files:**
- Delete: `docs/architecture/rust-migration.md`
- Modify: `website/sidebars.ts`, `docs/index.md`, `docs/architecture/index.md`, `docs/architecture/sdk-design.md`, `docs/architecture/hardening-design.md`, `docs/architecture/connectors.md`, `docs/guides/usage.md`, `docs/guides/memory.md`, `docs/guides/observability.md`, `docs/about/roadmap.md`

- [ ] **Step 1: Update architecture & index pages**

- Delete `docs/architecture/rust-migration.md` and remove `'architecture/rust-migration'` from `website/sidebars.ts`.
- Rewrite `docs/architecture/index.md` around the five levels of pieces.
- Update `docs/architecture/sdk-design.md` and `docs/architecture/hardening-design.md` status lines to **implemented**.
- Rewrite `docs/architecture/connectors.md` (`RecordStore` role, no `WalStore`, no `ConnectorRegistry`, no circuit breaker, `contract-suite` feature, `in_memory`).
- Update `docs/index.md` links and summary.

- [ ] **Step 2: Update guides and roadmap**

- Rewrite `docs/guides/usage.md` with one section per level embedding the tested code from `crates/aether-sdk/examples/level_*.rs`.
- Rewrite `docs/guides/memory.md` (record store + derived indexes, `User(UserId)` / `Session(SessionId)`, record-only session memory, `MemoryTier` as filterable label, `Buffered` durability, honest read-your-writes scoring).
- Update `docs/guides/observability.md` (`aether.cortex.index_pass`, `aether.cortex.index_record`, `aether.cortex.vector_search`, `aether.cortex.graph_search`, `record_*` ops).
- Update `docs/about/roadmap.md` (mark Phase 5 completed and update the status table).

- [ ] **Step 3: Build the docs site and run the gate**

Run: `cd website && npm run build`
Expected: build succeeds with no broken links or MDX errors.
Run the gate.

- [ ] **Step 4: Commit documentation updates in focused commits**

```bash
git add -A && git commit -m "docs: rewrite architecture, guides, and roadmap for pieces-first SDK"
```

---

### Task 15: Retire the Python package and transition note

**This task deletes code and must not start without explicit sign-off from the repository owner.**

**Files:**
- Delete: `src/`, `tests/`, `pyproject.toml`, `examples/quickstart.py`, `docs/architecture/change-overview.md`
- Modify: `.github/workflows/ci.yml`, `website/sidebars.ts`, `README.md`, `examples/README.md`, `docs/development/guidelines.md`, `docs/index.md`, `.gitignore`

- [ ] **Step 1: Confirm explicit owner sign-off and verify no remaining references in `crates/` or `docs/`**

Run: `grep -rn "aether_sdk\b\|pyproject\|src/aether_sdk" crates/ website/ docs/ .github/`
Expected: matches only in `.github/workflows/ci.yml` and `docs/development/guidelines.md`.

- [ ] **Step 2: Drop the Python job from `.github/workflows/ci.yml`, delete `src/`, `tests/`, `pyproject.toml`, `examples/quickstart.py`, and remove `docs/architecture/change-overview.md` (plus its link in `docs/index.md` and `website/sidebars.ts`)**

- [ ] **Step 3: Rewrite `README.md` for the pieces-first Rust SDK and replace `docs/development/guidelines.md` with Rust conventions (the gate, contract-suite rule, `Transient` vs `Permanent`, pointer to `AGENTS.md`)**

- [ ] **Step 4: Verify `cd website && npm run build` and the gate both pass, then commit**

```bash
git add -A && git commit -m "chore: retire Python package and finalize pieces-first Rust SDK docs"
```

---

## Spec Coverage Check

| Source requirement | Covered by |
|---|---|
| `change-overview.md` §3.1 Removals | Tasks 2, 9, 10, 11, 13 |
| `change-overview.md` §3.3 Repo cleanup | Task 15 |
| `change-overview.md` §3.4 Doc cleanup | Tasks 14, 15 |
| `change-overview.md` §4 Changes & §5 Additions | Tasks 1-13 |
| `sdk-design.md` §2 S1-S6 & §4.1-4.6 | Tasks 7, 10, 11, 13 |
| `hardening-design.md` G1-G6 | Tasks 4-12 |
