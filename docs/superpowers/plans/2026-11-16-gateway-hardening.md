# Gateway Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

> **Partly superseded (2026-11-16).** `docs/architecture/sdk-design.md` changed Aether's shape to pieces first after this plan was written.
> Tasks 1 to 7 and Task 14 are still valid, with one change: `Indexer` takes optional vector and graph stores.
> Tasks 8 to 13 must be revised against the SDK design before execution. Do not execute them as written.

**Goal:** Make the record store the source of truth and the vector and graph stores derived indexes, so that writes, updates, and deletes are never lost, never resurrected, and never mis-ranked.

**Architecture:** Every `remember`, `update`, and `forget` becomes a versioned change to one durable record in a `RecordStore`.
A single `Indexer` brings the vector and graph indexes up to each record's version and marks it indexed.
Recall resolves every hit through the record store, so missing, deleted, expired, and out-of-scope memories cannot be returned.

**Tech Stack:** Rust 2021, tokio, async-trait, serde, petgraph, tracing. No new dependencies.

**Spec:** `docs/architecture/hardening-design.md`

## Global Constraints

- No model calls, embedding, chunking, or extraction anywhere in the workspace.
- No new crate dependencies.
- Every behavioural change starts with a failing test (repo `AGENTS.md`, Red - Green - Refactor).
- Micro commits: one logical change per commit, conventional prefixes (`feat:`, `fix:`, `refactor:`, `test:`, `docs:`, `chore:`, `ci:`), workspace builds and passes tests at every commit.
- Do not add an agent co-author line to commit messages.
- Never use the em dash character in any file. Use a plain dash.
- In Markdown files, put each full sentence on its own line.
- Never edit `CHANGELOG.md` or auto-generated files.
- Unit tests never touch live external services.
- Metadata keys starting with `_aether_` are reserved for the gateway.
- After Task 1, every commit must pass `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, and `cargo test --workspace --all-features`.
  This triple is called **the gate** below.
- Docs under `docs/` are compiled as MDX. Keep generics such as `Arc<T>` and braces such as `{ a, b }` inside backticks or code fences.

## Review Focus

Inputs the design implies but does not spell out, most likely to bite first.
Each has a named test in the task that owns the code.

1. Two concurrent `remember` calls with the same idempotency key: exactly one record is created and both callers get the same `memory_id` (Task 5, Task 8).
2. A retried `create` for the same `MemoryId` without an idempotency key (a timeout followed by a retry): succeeds as `Created` and stores one record (Task 5).
3. `metadata` that is not a JSON object: rejected with `InvalidInput` instead of silently replaced by an empty object (Task 9).
4. A query vector containing `NaN` or infinity: rejected with `InvalidInput` (Task 9).
5. `recall` with `top_k = 0`: returns an empty result without error (Task 10).

## File Structure

| File | Responsibility | Change |
|------|----------------|--------|
| `.github/workflows/ci.yml` | CI | Add Rust job (Task 1), drop Python job (Task 14) |
| `crates/aether-core/src/lib.rs` | Contracts and DTOs | `RecordStore`, `IndexSpec`, scope ids, scoring and filter helpers |
| `crates/aether-adapters/src/memory_test.rs` | `InMemoryBackend` | New record-store state, fault injection |
| `crates/aether-adapters/src/middleware.rs` | Decorators | Follow trait changes, jitter |
| `crates/aether-adapters/src/contract.rs` | Reusable contract suite (new, feature `contract-suite`) | Generic functions any backend calls |
| `crates/aether-adapters/tests/contract_suite.rs` | Runs the suite on `InMemoryBackend` | Thin callers |
| `crates/aether-cortex/src/lib.rs` | Module wiring and re-exports only | Shrinks |
| `crates/aether-cortex/src/indexer.rs` | `Indexer` (new) | Replaces `IngestionBuffer` |
| `crates/aether-cortex/src/recall.rs` | `QueryPlanner` (moved) | Rewritten recall |
| `crates/aether-cortex/src/ppr.rs` | `PersonalizedPageRank` (moved) | Unchanged logic |
| `crates/aether-sdk/src/lib.rs` | `MemoryManager` facade | New write path, validation, builder defaults |
| `crates/aether-sdk/tests/hardening.rs` | Guarantee tests G1 to G5 through the public facade (new) | One test per guarantee |

Shared test fixtures used throughout (define once per test file that needs them):

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
- Modify: every `crates/**/*.rs` file touched by `cargo fmt` and clippy fixes

**Interfaces:**
- Produces: the gate commands that every later task runs.

- [ ] **Step 1: Format the workspace**

Run: `cargo fmt --all`
Then: `cargo fmt --all --check`
Expected: exits 0.

- [ ] **Step 2: Commit formatting alone**

```bash
git commit -am "style: apply rustfmt to the workspace"
```

- [ ] **Step 3: Fix clippy findings without changing behaviour**

Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
Today this reports about 22 errors: `map_or` that can be simplified (18), explicit `.into_iter()` (2), a collapsible `if` (1), and `sort_by` that should be `sort_by_key` (1).
Apply the suggested rewrites.
Do not add `#[allow]` attributes.
Expected after fixes: exits 0.

- [ ] **Step 4: Run the tests**

Run: `cargo test --workspace --all-features`
Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git commit -am "refactor: resolve clippy warnings across the workspace"
```

- [ ] **Step 6: Add the Rust job to `.github/workflows/ci.yml`**

Add a second job named `rust` beside the existing `test` job, on `ubuntu-latest`, with these steps in order: `actions/checkout@v4`, `dtolnay/rust-toolchain@stable` with `components: rustfmt, clippy`, `Swatinem/rust-cache@v2`, then the three gate commands as separate named steps.
Leave the Python job in place until Task 14.

- [ ] **Step 7: Commit**

```bash
git commit -am "ci: add rust fmt, clippy, and test job"
```

---

### Task 2: Move scoring and filter evaluation into `aether-core`

Two copies of filter evaluation and two copies of cosine similarity exist today (`aether-cortex/src/lib.rs` and `aether-adapters/src/memory_test.rs`).
Recall must score unflushed items with exactly the function the index uses, so there must be one.

**Files:**
- Modify: `crates/aether-core/src/lib.rs`
- Modify: `crates/aether-adapters/src/memory_test.rs` (delete `similarity`, `eval_filter`, `value_matches`)
- Modify: `crates/aether-cortex/src/lib.rs` (delete `cosine`, `eval_filter`, `value_matches`)
- Test: `crates/aether-core/tests/unit_types.rs`

**Interfaces:**
- Produces: `DistanceMetric::score(&self, a: &[f32], b: &[f32]) -> f32` (higher is closer; Euclidean returns negative distance, as the in-memory backend does today).
- Produces: `FilterExpr::matches(&self, metadata: &serde_json::Value) -> bool`.

- [ ] **Step 1: Write the failing tests**

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
Expected: compile error, no method `score` / `matches`.

- [ ] **Step 3: Implement both methods in `aether-core` by moving the existing bodies from `memory_test.rs`, then delete all four duplicate functions and call the new methods**

- [ ] **Step 4: Run the gate. Expected: pass**

- [ ] **Step 5: Commit**

```bash
git commit -am "refactor(core): single implementation of metric scoring and filter matching"
```

---

### Task 3: Rename `KvStore` to `RecordStore`

Structural only. No behaviour changes in this task.

**Files:**
- Modify: all four crates' `src/` and `tests/`

**Interfaces:**
- Produces: trait `RecordStore` (same methods as `KvStore` today), fields and parameters named `record_store`, `Traced` span `op` values `record_get`, `record_put`, `record_put_if_absent`, `record_cas_update`, `record_recent`, `record_delete`.

- [ ] **Step 1: Rename the trait, every `kv_store` identifier, and the `kv_*` span op strings**

`crates/aether-sdk/tests/tracing_otel.rs` asserts on `kv_put`; update it to `record_put`.
Rename the contract test `contract_kv_store_ttl_idempotency_cas_and_recent` to `contract_record_store_ttl_idempotency_cas_and_recent`.

- [ ] **Step 2: Confirm nothing was missed**

Run: `grep -rn "KvStore\|kv_store\|\"kv_" crates/`
Expected: no output.

- [ ] **Step 3: Run the gate. Expected: pass**

- [ ] **Step 4: Commit**

```bash
git commit -am "refactor: rename KvStore to RecordStore"
```

---

### Task 4: Reusable contract suite and fault injection

**Files:**
- Create: `crates/aether-adapters/src/contract.rs`
- Modify: `crates/aether-adapters/Cargo.toml`, `crates/aether-adapters/src/lib.rs`, `crates/aether-adapters/src/memory_test.rs`
- Modify: `crates/aether-adapters/tests/contract_suite.rs`

**Interfaces:**
- Produces: feature `contract-suite` on `aether-adapters`, gating `pub mod contract`.
- Produces: `contract::record_store<S: RecordStore>(store: &S)`, `contract::vector_store<S: VectorStore>(store: &S)`, `contract::graph_store<S: GraphStore>(store: &S)`, all `pub async fn`, panicking on violation.
  Each generates its own fresh `ProjectId`s so it can run against a shared real database.
- Produces: fault injection on `InMemoryBackend`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FaultPoint { VectorUpsert, VectorDelete, GraphUpsert, GraphRetract }

impl InMemoryBackend {
    /// The next `times` calls at `point` return `AetherError::Transient { connector: "in_memory", message: "injected fault".into() }`.
    pub fn fail_next(&self, point: FaultPoint, times: usize);
    /// Test inspection: whether a vector is stored for this id.
    pub fn has_vector(&self, ns: &ProjectId, id: MemoryId) -> bool;
}
```

`GraphUpsert` covers both `upsert_entities` and `upsert_relations`.

- [ ] **Step 1: Write the failing fault-injection test in `tests/contract_suite.rs`**

```rust
#[tokio::test]
async fn in_memory_backend_injects_transient_faults_then_recovers() {
    let b = InMemoryBackend::new();
    let p = ns();
    b.initialize(&p, 2, DistanceMetric::Cosine).await.unwrap();
    let rec = VectorRecord { id: MemoryId::new(), vector: vec![1.0, 0.0], metadata: json!({}) };
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
Expected: compile error, `FaultPoint` not found.

- [ ] **Step 3: Implement `FaultPoint`, `fail_next`, and `has_vector`**

Store remaining fault counts in a `Mutex<HashMap<FaultPoint, usize>>` on the backend and check it at the top of each affected method.

- [ ] **Step 4: Run the test. Expected: pass. Commit**

```bash
git commit -am "test(adapters): add fault injection to InMemoryBackend"
```

- [ ] **Step 5: Move the existing backend contract tests into `contract.rs` as the three generic functions**

Move the bodies of `contract_namespace_isolation_vector_and_graph`, `contract_upsert_delete_and_dimension_check`, `contract_filter_expr_evaluation`, `contract_temporal_provenance_and_retract_memory_facts`, and `contract_record_store_ttl_idempotency_cas_and_recent`, replacing `InMemoryBackend::new()` with the `store` parameter and fixed project ids with `ns()`.
`tests/contract_suite.rs` keeps one `#[tokio::test]` per generic function that calls it with `InMemoryBackend::new()`.
The middleware test `contract_resilient_retries_transient_only_and_batcher_chunks` and its `FlakyStore` stay in `tests/contract_suite.rs` unchanged.
In `Cargo.toml` add `contract-suite = []` under `[features]` and, under `[dev-dependencies]`, `aether-adapters = { path = ".", features = ["contract-suite"] }` so plain `cargo test` always runs the suite.

- [ ] **Step 6: Run the gate. Expected: pass, with the same number of contract assertions executed as before**

- [ ] **Step 7: Commit**

```bash
git commit -am "refactor(adapters): make the contract suite reusable across backends"
```

---

### Task 5: `RecordStore` versioned-record operations

Additive. The old methods (`put`, `put_if_absent`, `recent`, `delete`) stay until Task 8 removes them.

**Files:**
- Modify: `crates/aether-core/src/lib.rs` (trait, `Arc` forwarding impl, new types)
- Modify: `crates/aether-adapters/src/memory_test.rs`, `crates/aether-adapters/src/middleware.rs` (`Resilient`, `Timeout`, `Traced`)
- Modify: `crates/aether-adapters/src/contract.rs`

**Interfaces:**
- Produces, in `aether-core`:

```rust
#[derive(Debug, Clone, PartialEq)]
pub enum CreateOutcome { Created, Duplicate(MemoryReceipt) }

#[derive(Debug, Clone, PartialEq)]
pub struct PendingRecord { pub item: MemoryItem, pub tombstone: bool }

// added to RecordStore
async fn create(&self, ns: &ProjectId, item: &MemoryItem, ttl: Option<Duration>) -> Result<CreateOutcome>;
async fn get_many(&self, ns: &ProjectId, ids: &[MemoryId]) -> Result<Vec<MemoryItem>>;
async fn tombstone(&self, ns: &ProjectId, id: MemoryId) -> Result<Option<MemoryItem>>;
async fn pending(&self, limit: usize) -> Result<Vec<PendingRecord>>;
async fn pending_for(&self, ns: &ProjectId, limit: usize) -> Result<Vec<MemoryItem>>;
async fn mark_indexed(&self, ns: &ProjectId, id: MemoryId, version: u64) -> Result<()>;
```

Semantics the contract pins:

- `create` stores the item as pending and, when `item.idempotency_key` is set, binds the key in the same atomic step.
- `create` with a key already bound to a live record returns `Duplicate` with that record's `memory_id` and current `version`, and `state` `Persisted` if it is indexed, else `Buffered`.
- `create` with a `MemoryId` that already exists returns `Created` and leaves one record.
- `get` and `get_many` never return tombstoned or expired records. `get_many` omits misses and does not guarantee order.
- `cas_update` makes the record pending again.
- `tombstone` returns the record as it now stands (version bumped by one), or `None` if there is no live record. It releases the idempotency key immediately.
- `pending` returns live-pending and tombstoned records across all projects, oldest change first, never expired ones.
- `pending_for` returns live pending items of one project, never tombstones.
- `mark_indexed` is a no-op when `version` is not the record's current version. When it matches a tombstone, the record is removed.

- [ ] **Step 1: Add the new cases to `contract::record_store` (they fail to compile first)**

```rust
// create, pending, mark_indexed
let p = ns();
let a = item(&p, "a", Some(vec![1.0, 0.0]));
assert_eq!(store.create(&p, &a, None).await.unwrap(), CreateOutcome::Created);
assert_eq!(store.pending_for(&p, 10).await.unwrap().len(), 1);
assert!(store.pending(1000).await.unwrap().iter().any(|r| r.item.id == a.id && !r.tombstone));
store.mark_indexed(&p, a.id, 99).await.unwrap();                       // stale version: no-op
assert_eq!(store.pending_for(&p, 10).await.unwrap().len(), 1);
store.mark_indexed(&p, a.id, a.version).await.unwrap();
assert!(store.pending_for(&p, 10).await.unwrap().is_empty());
assert!(store.get(&p, a.id).await.unwrap().is_some());

// Review Focus 2: retried create for the same id
assert_eq!(store.create(&p, &a, None).await.unwrap(), CreateOutcome::Created);
assert_eq!(store.get_many(&p, &[a.id, MemoryId::new()]).await.unwrap().len(), 1);

// idempotency key bound atomically, released by tombstone
let mut k1 = item(&p, "k", None); k1.idempotency_key = Some("key-1".into());
let mut k2 = item(&p, "k again", None); k2.idempotency_key = Some("key-1".into());
assert_eq!(store.create(&p, &k1, None).await.unwrap(), CreateOutcome::Created);
match store.create(&p, &k2, None).await.unwrap() {
    CreateOutcome::Duplicate(r) => { assert_eq!(r.memory_id, k1.id); assert_eq!(r.state, ReceiptState::Buffered); }
    other => panic!("expected Duplicate, got {other:?}"),
}
assert!(store.get(&p, k2.id).await.unwrap().is_none());

// cas_update makes the record pending again
let mut a2 = a.clone(); a2.version = 2; a2.content = "a2".into();
store.cas_update(&p, a.id, Some(1), &a2).await.unwrap();
assert_eq!(store.pending_for(&p, 10).await.unwrap().iter().filter(|i| i.id == a.id).count(), 1);

// tombstone hides the record, keeps it pending, and is finalized by mark_indexed
let dead = store.tombstone(&p, k1.id).await.unwrap().expect("live record");
assert_eq!(dead.version, k1.version + 1);
assert!(store.get(&p, k1.id).await.unwrap().is_none());
assert!(store.pending_for(&p, 10).await.unwrap().iter().all(|i| i.id != k1.id));
assert!(store.pending(1000).await.unwrap().iter().any(|r| r.item.id == k1.id && r.tombstone));
assert_eq!(store.create(&p, &k2, None).await.unwrap(), CreateOutcome::Created);   // key was released
store.mark_indexed(&p, k1.id, dead.version).await.unwrap();
assert!(store.pending(1000).await.unwrap().iter().all(|r| r.item.id != k1.id));
assert!(store.tombstone(&p, MemoryId::new()).await.unwrap().is_none());

// expired records are neither readable nor pending
let e = item(&p, "ttl", None);
store.create(&p, &e, Some(Duration::from_millis(25))).await.unwrap();
tokio::time::sleep(Duration::from_millis(40)).await;
assert!(store.get(&p, e.id).await.unwrap().is_none());
assert!(store.pending(1000).await.unwrap().iter().all(|r| r.item.id != e.id));
```

Add a second generic function for Review Focus 1, because it needs a shared handle:

```rust
pub async fn record_store_concurrent_idempotency<S: RecordStore + 'static>(store: Arc<S>) {
    let p = ns();
    let mut tasks = Vec::new();
    for i in 0..16 {
        let (s, p) = (store.clone(), p.clone());
        tasks.push(tokio::spawn(async move {
            let mut it = item(&p, &format!("c{i}"), None);
            it.idempotency_key = Some("same".into());
            match s.create(&p, &it, None).await.unwrap() {
                CreateOutcome::Created => (true, it.id),
                CreateOutcome::Duplicate(r) => (false, r.memory_id),
            }
        }));
    }
    let mut results: Vec<(bool, MemoryId)> = Vec::new();
    for t in tasks { results.push(t.await.unwrap()); }
    assert_eq!(results.iter().filter(|(created, _)| *created).count(), 1);
    assert!(results.iter().all(|(_, id)| *id == results[0].1));
}
```

Call it from `tests/contract_suite.rs` under `#[tokio::test(flavor = "multi_thread")]`.

- [ ] **Step 2: Run and confirm failure**

Run: `cargo test -p aether-adapters`
Expected: compile error, no method `create`.

- [ ] **Step 3: Add the types and trait methods to `aether-core` and the `Arc` forwarding impl**

- [ ] **Step 4: Implement in `InMemoryBackend`**

Extend `KvEntry` (rename it `RecordEntry`) with `indexed_version: u64` (0 when never indexed) and `tombstone: bool`.
A record is pending when `indexed_version != item.version`.
`seq` is reassigned from `next_seq` on every change and orders `pending`.
`create` must do the key check and the insert under one write lock.

- [ ] **Step 5: Add delegations in `Resilient`, `Timeout`, and `Traced`**

`Traced` op names: `record_create`, `record_get_many`, `record_tombstone`, `record_pending`, `record_pending_for`, `record_mark_indexed`.
`record_pending` has no `project_id` field.

- [ ] **Step 6: Run the gate. Expected: pass**

- [ ] **Step 7: Commit in dependency order**

```bash
git commit -m "feat(core): add versioned record operations to RecordStore"
git commit -m "feat(adapters): implement versioned records in InMemoryBackend and middleware"
```

The first commit must compile on its own: give the new trait methods no default bodies, and include the backend and middleware implementations in the same commit if splitting would break the build.
A single commit is acceptable here.

---

### Task 6: Idempotent relation upserts in `GraphStore`

The indexer retracts a memory's facts and then upserts its current ones on every pass.
Today `upsert_relations` appends a new edge each call, so a retried pass would duplicate facts.

**Files:**
- Modify: `crates/aether-adapters/src/memory_test.rs`, `crates/aether-adapters/src/contract.rs`
- Modify: `crates/aether-core/src/lib.rs` (doc comment on `GraphStore::upsert_relations` only)

**Interfaces:**
- Produces: contract rule. An edge is keyed by (`source_memory_id`, `source`, `target`, `relation_type`). `upsert_relations` replaces an existing edge with that key, including a retracted one, instead of adding a second.

- [ ] **Step 1: Add the failing case to `contract::graph_store`**

```rust
let p = ns();
let m = MemoryId::new();
let rel = Relation {
    source: EntityId::new("person:alice").unwrap(), target: EntityId::new("db:qdrant").unwrap(),
    relation_type: "USES".into(), valid_at: Utc::now(), invalid_at: None,
};
let seeds = [rel.source.clone()];
let active = TemporalFilter::default();
store.upsert_relations(&p, m, &[rel.clone()]).await.unwrap();
store.upsert_relations(&p, m, &[rel.clone()]).await.unwrap();
assert_eq!(store.neighborhood(&p, &seeds, 1, 25, &active).await.unwrap().edges.len(), 1);
store.retract_memory_facts(&p, m, Utc::now()).await.unwrap();
assert!(store.neighborhood(&p, &seeds, 1, 25, &active).await.unwrap().edges.is_empty());
store.upsert_relations(&p, m, &[rel]).await.unwrap();
let all = TemporalFilter { as_of: None, active_only: false };
assert_eq!(store.neighborhood(&p, &seeds, 1, 25, &active).await.unwrap().edges.len(), 1);
assert_eq!(store.neighborhood(&p, &seeds, 1, 25, &all).await.unwrap().edges.len(), 1);
```

- [ ] **Step 2: Run and confirm failure**

Run: `cargo test -p aether-adapters --test contract_suite`
Expected: FAIL, left `2`, right `1`.

- [ ] **Step 3: Make `InMemoryBackend::upsert_relations` replace the edge weight when an edge with the same key exists between the two nodes; document the rule on the trait method**

- [ ] **Step 4: Run the gate. Expected: pass**

- [ ] **Step 5: Commit**

```bash
git commit -am "fix(adapters): make relation upserts idempotent per source memory"
```

---

### Task 7: `Indexer`

**Files:**
- Create: `crates/aether-cortex/src/indexer.rs`
- Modify: `crates/aether-cortex/src/lib.rs` (`mod indexer; pub use indexer::*;`)
- Test: `crates/aether-cortex/tests/unit_indexer.rs` (new)

**Interfaces:**
- Consumes: `RecordStore::{pending, mark_indexed}`, `PendingRecord`, `FaultPoint`, `InMemoryBackend::{fail_next, has_vector}`.
- Produces:

```rust
/// Metadata written to the vector index for an item: the caller's metadata object
/// plus `_aether_tier` (the serde snake_case name of the tier).
pub fn index_metadata(item: &MemoryItem) -> serde_json::Value;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PassReport { pub indexed: usize, pub failed: usize }

pub struct Indexer { /* record, vector, graph: Arc<dyn ...> */ }

impl Indexer {
    pub fn new(record: Arc<dyn RecordStore>, vector: Arc<dyn VectorStore>, graph: Arc<dyn GraphStore>) -> Self;
    /// Brings the indexes to this record's version, then marks it indexed.
    pub async fn apply(&self, record: &PendingRecord) -> Result<()>;
    /// Applies pending records until none remain or only failing ones remain.
    /// Returns `Err` with the last failure if any record failed.
    pub async fn run_pass(&self) -> Result<PassReport>;
}
```

`apply`, in order:

1. Tombstone: `vector.delete([id])`, `graph.retract_memory_facts(id, now)`, `mark_indexed`.
2. Live: if `embedding` is `Some`, `vector.upsert_batch` one `VectorRecord` with `index_metadata(item)`; otherwise `vector.delete([id])`.
3. Live: `graph.retract_memory_facts(id, now)`, then `upsert_entities` and `upsert_relations` when non-empty.
4. `mark_indexed(ns, id, item.version)`.

`content` is no longer written into vector metadata.
`run_pass` reads `pending(256)` in a loop, skips ids that already failed in this pass, and stops when a batch yields nothing new.
Mark that loop with this comment: `// ponytail: a pass stalls if 256+ records fail permanently; add a cursor to pending() if that ever happens.`
Spans: `aether.cortex.index_pass` (fields `indexed`, `failed`) and `aether.cortex.index_record` (fields `project_id`, `memory_id`, `tombstone`).

- [ ] **Step 1: Write the failing tests**

```rust
async fn setup() -> (Arc<InMemoryBackend>, Indexer, ProjectId) {
    let b = Arc::new(InMemoryBackend::new());
    let p = ns();
    b.initialize(&p, 2, DistanceMetric::Cosine).await.unwrap();
    (b.clone(), Indexer::new(b.clone(), b.clone(), b), p)
}

#[tokio::test]
async fn failed_index_write_keeps_the_record_pending_and_indexes_the_rest() {
    let (b, ix, p) = setup().await;
    let (a, c) = (item(&p, "a", Some(vec![1.0, 0.0])), item(&p, "c", Some(vec![0.0, 1.0])));
    b.create(&p, &a, None).await.unwrap();
    b.create(&p, &c, None).await.unwrap();
    b.fail_next(FaultPoint::VectorUpsert, 1);
    assert!(ix.run_pass().await.is_err());
    assert_eq!(b.pending_for(&p, 10).await.unwrap().len(), 1);     // one failed, one indexed
    assert_eq!(ix.run_pass().await.unwrap(), PassReport { indexed: 1, failed: 0 });
    assert!(b.has_vector(&p, a.id) && b.has_vector(&p, c.id));
    assert!(b.pending_for(&p, 10).await.unwrap().is_empty());
}

#[tokio::test]
async fn tombstone_removes_vector_and_facts_then_the_record() {
    let (b, ix, p) = setup().await;
    let mut a = item(&p, "a", Some(vec![1.0, 0.0]));
    a.relations = vec![/* person:alice USES db:qdrant, valid now */];
    b.create(&p, &a, None).await.unwrap();
    ix.run_pass().await.unwrap();
    b.tombstone(&p, a.id).await.unwrap();
    b.fail_next(FaultPoint::VectorDelete, 1);
    assert!(ix.run_pass().await.is_err());
    assert!(b.pending(100).await.unwrap().iter().any(|r| r.item.id == a.id));   // retried, not orphaned
    ix.run_pass().await.unwrap();
    assert!(!b.has_vector(&p, a.id));
    let seeds = [EntityId::new("person:alice").unwrap()];
    assert!(b.neighborhood(&p, &seeds, 1, 25, &TemporalFilter::default()).await.unwrap().edges.is_empty());
    assert!(b.pending(100).await.unwrap().is_empty());
}

#[tokio::test]
async fn item_without_embedding_has_its_stale_vector_removed() {
    let (b, ix, p) = setup().await;
    let a = item(&p, "a", Some(vec![1.0, 0.0]));
    b.create(&p, &a, None).await.unwrap();
    ix.run_pass().await.unwrap();
    let mut a2 = a.clone(); a2.version = 2; a2.embedding = None;
    b.cas_update(&p, a.id, Some(1), &a2).await.unwrap();
    ix.run_pass().await.unwrap();
    assert!(!b.has_vector(&p, a.id));
}

#[test]
fn index_metadata_adds_tier_and_omits_content() {
    let mut it = item(&ns(), "secret text", None);
    it.metadata = serde_json::json!({"k": "v"});
    let m = index_metadata(&it);
    assert_eq!(m["k"], "v");
    assert_eq!(m["_aether_tier"], "semantic");
    assert!(m.get("content").is_none());
}
```

- [ ] **Step 2: Run and confirm failure**

Run: `cargo test -p aether-cortex --test unit_indexer`
Expected: compile error, `Indexer` not found.

- [ ] **Step 3: Implement `indexer.rs` as specified above**

- [ ] **Step 4: Run the gate. Expected: pass**

- [ ] **Step 5: Commit**

```bash
git commit -am "feat(cortex): add Indexer that derives vector and graph indexes from records"
```

---

### Task 8: Route `MemoryManager` through records and the indexer

This is the behavioural switch.
It lands as three commits: the switch, then two pure deletions.

**Files:**
- Modify: `crates/aether-sdk/src/lib.rs`
- Modify: `crates/aether-cortex/src/lib.rs` (planner read path; then deletions)
- Create: `crates/aether-sdk/tests/hardening.rs`
- Modify: `crates/aether-sdk/tests/*.rs`, `crates/aether-cortex/tests/unit_cortex.rs`, `crates/aether-core/src/lib.rs`, `crates/aether-adapters/src/{memory_test,middleware,contract}.rs`

**Interfaces:**
- Consumes: `Indexer::{new, apply, run_pass}`, `RecordStore::{create, get_many, tombstone, pending_for}`.
- Produces: `MemoryManagerBuilder::with_index_interval(self, interval: Option<Duration>) -> Self`, default `Some(Duration::from_secs(1))`. `None` disables the background loop (tests use this to control timing).
- Produces: `QueryPlanner::new(record_store, vector_store, graph_store)` (three arguments; no cache, no buffer).

New manager behaviour:

- `remember`: `input.validate()`, build the item, `record_store.create`. `Duplicate(r)` returns `r`. `Buffered` returns `ReceiptState::Buffered` and wakes the loop. `WaitFlush` calls `indexer.apply` for this record and returns `Persisted`, or the error.
- `update`: unchanged read and patch logic, `cas_update`, then `indexer.apply`.
- `forget`: `tombstone`; `None` returns `Ok(())`; otherwise `indexer.apply` on the tombstone.
- `flush`: `indexer.run_pass()`, mapping the report away.
- `shutdown`: stop the loop, `run_pass`, close all three stores. A `remember` after shutdown returns `InvalidInput("memory manager is shut down")`.
- Background loop: started lazily on the first `Buffered` write with `tokio::spawn`, never in `build()` (which may run outside a runtime). It waits on a `tokio::sync::Notify` or the interval, calls `run_pass`, and logs failures with `tracing::warn!`. It holds a `Weak` to the indexer and exits when the manager is dropped.

Planner read path for this task (ranking is rewritten in Task 10):

- Vector hits are resolved with `get_many`; hits with no record are dropped; `RecallHit.content` and `metadata` come from the record.
- Read-your-writes candidates come from `pending_for(project, 256)` only. Their scoring stays as it is until Task 10.
- Seed lookup by entity-name substring uses the `pending_for` items.

- [ ] **Step 1: Write the failing guarantee tests in `crates/aether-sdk/tests/hardening.rs`**

```rust
async fn setup() -> (Arc<InMemoryBackend>, MemoryManager, ProjectId) {
    let b = Arc::new(InMemoryBackend::new());
    let p = ns();
    b.initialize(&p, 2, DistanceMetric::Cosine).await.unwrap();
    let m = MemoryManager::builder(b.clone(), b.clone(), b.clone()).with_index_interval(None).build();
    (b, m, p)
}
fn input(p: &ProjectId, content: &str, v: [f32; 2]) -> MemoryInput {
    let mut i = MemoryInput::new(p.clone(), content); i.embedding = Some(v.to_vec()); i
}
fn vquery(p: &ProjectId, v: [f32; 2]) -> RecallQuery {
    let mut q = RecallQuery::new(p.clone(), "q"); q.query_vector = Some(v.to_vec()); q.mode = RecallMode::Vector; q
}

#[tokio::test]   // G1
async fn failed_flush_loses_nothing() {
    let (b, m, p) = setup().await;
    let r = m.remember(input(&p, "a", [1.0, 0.0])).await.unwrap();
    b.fail_next(FaultPoint::VectorUpsert, 1);
    assert!(m.flush().await.is_err());
    m.flush().await.unwrap();
    assert!(b.has_vector(&p, r.memory_id));
}

#[tokio::test]   // G1
async fn buffered_writes_survive_a_rebuilt_manager() {
    let (b, m, p) = setup().await;
    let r = m.remember(input(&p, "a", [1.0, 0.0])).await.unwrap();
    drop(m);                                                     // simulated crash before any flush
    let m2 = MemoryManager::builder(b.clone(), b.clone(), b.clone()).with_index_interval(None).build();
    m2.flush().await.unwrap();
    assert!(b.has_vector(&p, r.memory_id));
}

#[tokio::test]   // G1
async fn buffered_writes_are_indexed_without_an_explicit_flush() {
    let b = Arc::new(InMemoryBackend::new());
    let p = ns();
    b.initialize(&p, 2, DistanceMetric::Cosine).await.unwrap();
    let m = MemoryManager::builder(b.clone(), b.clone(), b.clone())
        .with_index_interval(Some(Duration::from_millis(20))).build();
    let r = m.remember(input(&p, "a", [1.0, 0.0])).await.unwrap();
    assert_eq!(r.state, ReceiptState::Buffered);
    tokio::time::timeout(Duration::from_secs(2), async {
        while !b.has_vector(&p, r.memory_id) { tokio::time::sleep(Duration::from_millis(5)).await; }
    }).await.expect("background indexer never ran");
}

#[tokio::test]   // G2
async fn forgetting_an_unflushed_memory_does_not_resurrect_it() {
    let (b, m, p) = setup().await;
    let r = m.remember(input(&p, "a", [1.0, 0.0])).await.unwrap();
    m.forget(&p, r.memory_id).await.unwrap();
    m.flush().await.unwrap();
    assert!(!b.has_vector(&p, r.memory_id));
    assert!(m.recall(vquery(&p, [1.0, 0.0])).await.unwrap().hits.is_empty());
}

#[tokio::test]   // G2
async fn failed_forget_hides_the_memory_and_is_retried() {
    let (b, m, p) = setup().await;
    let mut i = input(&p, "a", [1.0, 0.0]); i.durability = WriteDurability::WaitFlush;
    let r = m.remember(i).await.unwrap();
    b.fail_next(FaultPoint::VectorDelete, 1);
    assert!(m.forget(&p, r.memory_id).await.is_err());
    assert!(b.has_vector(&p, r.memory_id));                                   // orphan exists for now
    assert!(m.recall(vquery(&p, [1.0, 0.0])).await.unwrap().hits.is_empty()); // but is never returned
    m.flush().await.unwrap();
    assert!(!b.has_vector(&p, r.memory_id));
}

#[tokio::test(flavor = "multi_thread")]   // Review Focus 1
async fn concurrent_remembers_with_one_key_create_one_memory() {
    let (_b, m, p) = setup().await;
    let m = Arc::new(m);
    let mut handles = Vec::new();
    for n in 0..16 {
        let (m, p) = (m.clone(), p.clone());
        handles.push(tokio::spawn(async move {
            let mut i = input(&p, &format!("c{n}"), [1.0, 0.0]); i.idempotency_key = Some("same".into());
            m.remember(i).await.unwrap().memory_id
        }));
    }
    let mut ids = Vec::new();
    for h in handles { ids.push(h.await.unwrap()); }
    assert!(ids.iter().all(|id| *id == ids[0]));
    assert_eq!(m.recall(vquery(&p, [1.0, 0.0])).await.unwrap().hits.len(), 1);
}

#[tokio::test]
async fn forget_of_a_missing_memory_is_ok_and_update_is_not_found() {
    let (_b, m, p) = setup().await;
    m.forget(&p, MemoryId::new()).await.unwrap();
    let patch = MemoryUpdate { content: None, embedding: None, metadata: None, expected_version: None };
    assert!(matches!(m.update(&p, MemoryId::new(), patch).await, Err(AetherError::NotFound(_))));
}
```

- [ ] **Step 2: Run and confirm failure**

Run: `cargo test -p aether-sdk --test hardening`
Expected: compile error on `with_index_interval`; after stubbing it, `failed_flush_loses_nothing` and `forgetting_an_unflushed_memory_does_not_resurrect_it` fail.

- [ ] **Step 3: Implement the manager and planner changes described above**

- [ ] **Step 4: Update existing tests to the new behaviour**

`tracing_otel.rs`: replace the `aether.cortex.enqueue` and `aether.cortex.flush` assertions with `aether.cortex.index_record` parented by `aether.remember`, and `record_put` with `record_create`.
`unit_cortex.rs`: rewrite `ingestion_buffer_and_query_planner_support_read_your_writes_and_flush` against `Indexer` and the three-argument planner.

- [ ] **Step 5: Run the gate. Expected: pass. Commit**

```bash
git commit -am "feat(sdk): write, update, and forget through versioned records and the indexer"
```

- [ ] **Step 6: Delete `IngestionBuffer`, `HotContextCache`, `PolicyRouter`, `PolicyDecision`, `enrich_metadata`, and their tests in `unit_cortex.rs`**

Run: `grep -rn "IngestionBuffer\|HotContextCache\|PolicyRouter\|PolicyDecision" crates/`
Expected: no output.
Run the gate, then:

```bash
git commit -am "refactor(cortex): remove IngestionBuffer, HotContextCache, and PolicyRouter"
```

- [ ] **Step 7: Delete `RecordStore::{put, put_if_absent, recent, delete}` from the trait, the `Arc` impl, `InMemoryBackend`, the three middlewares, and the old contract cases that exercise them**

Keep the TTL, namespace-isolation, and CAS-conflict contract cases by rewriting them on `create`.
Run the gate, then:

```bash
git commit -am "refactor(core): remove superseded RecordStore methods"
```

---

### Task 9: Validation at the inbound boundary

**Files:**
- Modify: `crates/aether-core/src/lib.rs`, `crates/aether-adapters/src/{memory_test,middleware,contract}.rs`, `crates/aether-sdk/src/lib.rs`
- Test: `crates/aether-core/tests/unit_types.rs`, `crates/aether-sdk/tests/hardening.rs`

**Interfaces:**
- Produces:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexSpec { pub dimensions: usize, pub metric: DistanceMetric }

// added to VectorStore; None when the project's index was never initialized
async fn describe(&self, ns: &ProjectId) -> Result<Option<IndexSpec>>;

// MemoryUpdate gains
pub entities: Option<Vec<Entity>>,
pub relations: Option<Vec<Relation>>,

impl RecallQuery { pub fn validate(&self) -> Result<()>; }
```

- Produces: `MemoryManager` caches `IndexSpec` per project in an `RwLock<HashMap<ProjectId, IndexSpec>>` after the first `Some`, and exposes a private `async fn spec(&self, ns: &ProjectId) -> Result<IndexSpec>` that returns `InvalidInput("vector index is not initialized for project <id>")` on `None`.

Rules, all `AetherError::InvalidInput`:

| Where | Rule |
|-------|------|
| `MemoryInput::validate` | `metadata` must be a JSON object |
| `MemoryInput::validate` | no metadata key starts with `_aether_` |
| `remember`, `update` | embedding length equals `spec.dimensions` (checked only when an embedding is present) |
| `update` | patch vectors are non-empty and finite |
| `update` | new `content` without a new `embedding`, on an item that has an embedding |
| `update` | patch `metadata` obeys the two metadata rules |
| `RecallQuery::validate` | `Vector` or `Hybrid` mode without `query_vector` |
| `RecallQuery::validate` | `Graph` mode with empty `seed_entities` |
| `RecallQuery::validate` | `query_vector` empty or containing a non-finite value |
| `recall` | `query_vector` length equals `spec.dimensions` |

`update` applies `entities` and `relations` when present; the indexer already retracts and re-upserts facts.

- [ ] **Step 1: Write the failing tests**

In `unit_types.rs`:

```rust
#[test]
fn memory_input_rejects_non_object_and_reserved_metadata() {     // Review Focus 3
    let mut i = input_fixture(); i.metadata = serde_json::json!("a string");
    assert!(matches!(i.validate(), Err(AetherError::InvalidInput(_))));
    let mut i = input_fixture(); i.metadata = serde_json::json!({"_aether_scope": "project"});
    assert!(matches!(i.validate(), Err(AetherError::InvalidInput(_))));
}

#[test]
fn recall_query_validation() {
    let p = ProjectId::new("p").unwrap();
    let q = RecallQuery::new(p.clone(), "q");                                  // Hybrid, no vector
    assert!(q.validate().is_err());
    let mut g = RecallQuery::new(p.clone(), "q"); g.mode = RecallMode::Graph;  // no seeds
    assert!(g.validate().is_err());
    let mut n = RecallQuery::new(p, "q"); n.query_vector = Some(vec![f32::NAN, 0.0]);   // Review Focus 4
    assert!(n.validate().is_err());
}
```

In `hardening.rs`:

```rust
#[tokio::test]   // G5
async fn wrong_dimension_is_rejected_at_remember_and_nothing_is_stored() {
    let (b, m, p) = setup().await;
    let mut i = MemoryInput::new(p.clone(), "a"); i.embedding = Some(vec![1.0, 0.0, 0.0]);
    assert!(matches!(m.remember(i).await, Err(AetherError::InvalidInput(_))));
    assert!(b.pending(100).await.unwrap().is_empty());
    let mut q = vquery(&p, [1.0, 0.0]); q.query_vector = Some(vec![1.0]);
    assert!(matches!(m.recall(q).await, Err(AetherError::InvalidInput(_))));
}

#[tokio::test]   // G5
async fn embedding_for_an_uninitialized_project_is_rejected() {
    let (_b, m, _p) = setup().await;
    assert!(matches!(m.remember(input(&ns(), "a", [1.0, 0.0])).await, Err(AetherError::InvalidInput(_))));
}

#[tokio::test]
async fn update_content_requires_a_new_embedding_and_can_replace_facts() {
    let (b, m, p) = setup().await;
    let mut i = input(&p, "a", [1.0, 0.0]); i.durability = WriteDurability::WaitFlush;
    i.relations = vec![/* person:alice USES db:qdrant */];
    let r = m.remember(i).await.unwrap();
    let base = MemoryUpdate { content: None, embedding: None, metadata: None, entities: None, relations: None, expected_version: None };
    let no_vec = MemoryUpdate { content: Some("b".into()), ..base.clone() };
    assert!(matches!(m.update(&p, r.memory_id, no_vec).await, Err(AetherError::InvalidInput(_))));
    let swap = MemoryUpdate { relations: Some(vec![/* person:alice USES db:neo4j */]), ..base };
    m.update(&p, r.memory_id, swap).await.unwrap();
    let seeds = [EntityId::new("person:alice").unwrap()];
    let edges = b.neighborhood(&p, &seeds, 1, 25, &TemporalFilter::default()).await.unwrap().edges;
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].target.as_str(), "db:neo4j");
}
```

Add to `contract::vector_store`: `describe` is `None` before `initialize` and `Some(IndexSpec { dimensions: 3, metric: DistanceMetric::Cosine })` after `initialize(ns, 3, Cosine)`.

- [ ] **Step 2: Run and confirm failure**

Run: `cargo test --workspace`
Expected: compile errors on `describe`, `RecallQuery::validate`, and the new `MemoryUpdate` fields.

- [ ] **Step 3: Implement the types, `describe` (backend, `Arc` impl, `Resilient`, `Timeout`, `Batcher`, `Traced` with op `describe`), the validators, and the manager checks**

Existing tests that recall in `Hybrid` mode without a vector, or build `MemoryUpdate` without the new fields, must be updated to stay meaningful, not deleted.

- [ ] **Step 4: Run the gate. Expected: pass**

- [ ] **Step 5: Commit**

```bash
git commit -m "feat(core): add IndexSpec, VectorStore::describe, and query validation"
git commit -m "feat(sdk): reject invalid vectors, metadata, and updates at the boundary"
```

---

### Task 10: Recall ranking

**Files:**
- Create: `crates/aether-cortex/src/recall.rs`, `crates/aether-cortex/src/ppr.rs` (move `QueryPlanner` and `PersonalizedPageRank` out of `lib.rs` first, as a separate `refactor:` commit with no logic change)
- Test: `crates/aether-cortex/tests/unit_recall.rs` (new), `crates/aether-sdk/tests/hardening.rs`

**Interfaces:**
- Consumes: `DistanceMetric::score`, `FilterExpr::matches`, `index_metadata`, `IndexSpec`.
- Produces: `QueryPlanner::plan_and_execute(&self, query: &RecallQuery, spec: Option<IndexSpec>) -> Result<RecallResult>`. The manager passes `Some(spec)` whenever the query has a vector.
- Produces: `pub const RRF_K: f32 = 60.0;` in `recall.rs`.

Algorithm (the manager has already validated the query):

1. `top_k == 0` returns `RecallResult::default()` immediately.
2. **Vector ranking** (`Vector`, `Hybrid`): search with `top_k` and the caller filter, resolve through `get_many`, drop unresolved hits. Score is the index's score.
3. **Read-your-writes** (`ReadYourWrites` only): for each `pending_for(project, 256)` item not already a hit, keep it if `filters` is `None` or `filters.matches(&index_metadata(item))`, and if it has an embedding of the query's length. Score is `spec.metric.score(query_vector, embedding)`. Items without a comparable embedding are left out. A pending item that is also an index hit replaces the hit, so the caller sees its own latest version.
4. Sort the vector ranking by score descending.
5. **Graph ranking** (`Graph`, `Hybrid`): seeds are `seed_entities` plus, in `Hybrid`, the entity ids of the vector ranking's items (their `entities` and both endpoints of their `relations`). Empty seeds in `Hybrid` skips the graph. Fetch `neighborhood`, run PPR over it, and for every edge add `ppr[edge.source] + ppr[edge.target]` to `graph_score[edge.source_memory_id]`. Resolve those memory ids through `get_many`, drop unresolved, sort descending.
6. **Final hits**: `Vector` returns the vector ranking. `Graph` returns the graph ranking with the graph score. `Hybrid` scores each memory `sum over the rankings it appears in of 1.0 / (RRF_K + rank)` with rank starting at 1, sorted descending.
   Equal scores are ordered by `memory_id` so results are deterministic.
7. Truncate to `top_k`. `subgraph` is the neighbourhood from step 5, or empty.

Delete the constant `0.75` and `0.5` scores and the entity-name substring seed lookup.

- [ ] **Step 1: Move the two types into their own files; run the gate; commit**

```bash
git commit -am "refactor(cortex): split recall and ppr into modules"
```

- [ ] **Step 2: Write the failing tests**

In `hardening.rs`:

```rust
#[tokio::test]   // G3
async fn unflushed_and_flushed_copies_score_the_same() {
    let (_b, m, p) = setup().await;
    m.remember(input(&p, "a", [0.6, 0.8])).await.unwrap();
    let before = m.recall(vquery(&p, [1.0, 0.0])).await.unwrap().hits[0].score;
    m.flush().await.unwrap();
    let after = m.recall(vquery(&p, [1.0, 0.0])).await.unwrap().hits[0].score;
    assert!((before - 0.6).abs() < 1e-5 && (after - 0.6).abs() < 1e-5);
}

#[tokio::test]   // G3
async fn an_unrelated_unflushed_write_does_not_outrank_a_real_match() {
    let (_b, m, p) = setup().await;
    let mut near = input(&p, "near", [1.0, 0.0]); near.durability = WriteDurability::WaitFlush;
    let near_id = m.remember(near).await.unwrap().memory_id;
    m.remember(input(&p, "far", [0.0, 1.0])).await.unwrap();                 // buffered
    m.remember(MemoryInput::new(p.clone(), "no embedding")).await.unwrap();  // buffered, unscorable
    let hits = m.recall(vquery(&p, [1.0, 0.0])).await.unwrap().hits;
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].memory_id, near_id);
}

#[tokio::test]
async fn eventual_consistency_skips_unflushed_writes() {
    let (_b, m, p) = setup().await;
    m.remember(input(&p, "a", [1.0, 0.0])).await.unwrap();
    let mut q = vquery(&p, [1.0, 0.0]); q.consistency = ConsistencyMode::Eventual;
    assert!(m.recall(q).await.unwrap().hits.is_empty());
}

#[tokio::test]   // Review Focus 5
async fn top_k_zero_returns_empty() {
    let (_b, m, p) = setup().await;
    m.remember(input(&p, "a", [1.0, 0.0])).await.unwrap();
    let mut q = vquery(&p, [1.0, 0.0]); q.top_k = 0;
    assert!(m.recall(q).await.unwrap().hits.is_empty());
}

#[tokio::test]
async fn hybrid_surfaces_a_graph_connected_memory_the_vector_search_missed() {
    let (_b, m, p) = setup().await;
    // A: vector [1,0], relation person:alice USES db:qdrant
    // B: vector [0,1], relation db:qdrant HOSTED_ON cloud:aws   (far from the query, linked through db:qdrant)
    // C: vector [-1,0], no relations                            (far from the query, not linked)
    // all WaitFlush
    let mut q = vquery(&p, [1.0, 0.0]); q.mode = RecallMode::Hybrid; q.top_k = 3;
    let hits = m.recall(q).await.unwrap().hits;
    let pos = |id| hits.iter().position(|h| h.memory_id == id).unwrap();
    assert!(pos(b) < pos(c));          // B and C are both far by vector; only the graph link separates them
    assert!(pos(a) < pos(c));
}
```

In `unit_recall.rs`, one test that `Graph` mode with a seed returns the memory that sourced the seed's edge and no unrelated memory.

- [ ] **Step 3: Run and confirm failure**

Run: `cargo test -p aether-sdk --test hardening`
Expected: `unflushed_and_flushed_copies_score_the_same` fails (before is `0.5`), `an_unrelated_unflushed_write_does_not_outrank_a_real_match` fails (3 hits).

- [ ] **Step 4: Implement the algorithm in `recall.rs` and pass `spec` from the manager**

- [ ] **Step 5: Run the gate. Expected: pass**

- [ ] **Step 6: Commit**

```bash
git commit -am "feat(cortex): rank recall with real scores and fuse graph results"
```

---

### Task 11: Scopes with identity, record-only session memory

**Files:**
- Modify: `crates/aether-core/src/lib.rs`, `crates/aether-adapters/src/{memory_test,middleware,contract}.rs`, `crates/aether-cortex/src/{indexer,recall}.rs`, `crates/aether-sdk/src/lib.rs`
- Test: `crates/aether-core/tests/unit_types.rs`, `crates/aether-sdk/tests/hardening.rs`

**Interfaces:**
- Produces:

```rust
pub struct UserId(String);      // same shape as EntityId: new() validates non-empty, as_str(), Display, serde transparent
pub struct SessionId(String);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum MemoryScope { #[default] Project, User(UserId), Session(SessionId) }

impl MemoryScope {
    /// "project" or "user:<id>". None for sessions, which are never indexed.
    pub fn index_key(&self) -> Option<String>;
}

// RecallQuery gains (default vec![MemoryScope::Project]); validate() rejects an empty list
pub scopes: Vec<MemoryScope>,

// added to RecordStore
async fn session_items(&self, ns: &ProjectId, session: &SessionId, limit: usize) -> Result<Vec<MemoryItem>>;
```

`MemoryScope` is no longer `Copy`, and `Global` is removed.

Behaviour:

- `index_metadata` also writes `_aether_scope` with `index_key()`.
- `Indexer::apply` on a live `Session` item skips index writes and only calls `mark_indexed`.
- Recall ANDs `FilterExpr::In { field: "_aether_scope", values: <index keys of query.scopes> }` into the caller filter. If no requested scope has an index key, the vector search is skipped.
- Read-your-writes candidates and graph-ranked memories are dropped unless `query.scopes` contains their scope.
- For each `Session(s)` in `query.scopes`, `session_items(project, s, 256)` are added as candidates and scored exactly like read-your-writes candidates, regardless of consistency mode.
- `session_items` returns live, unexpired items only (pending or indexed).

- [ ] **Step 1: Write the failing tests**

In `unit_types.rs`:

```rust
#[test]
fn memory_scope_serde_and_index_key() {
    let u = MemoryScope::User(UserId::new("u1").unwrap());
    assert_eq!(serde_json::to_value(&u).unwrap(), serde_json::json!({"kind": "user", "id": "u1"}));
    assert_eq!(serde_json::to_value(MemoryScope::Project).unwrap(), serde_json::json!({"kind": "project"}));
    assert_eq!(u.index_key().as_deref(), Some("user:u1"));
    assert_eq!(MemoryScope::Session(SessionId::new("s").unwrap()).index_key(), None);
    assert!(UserId::new(" ").is_err());
}
```

In `hardening.rs`:

```rust
#[tokio::test]   // G4
async fn one_user_cannot_recall_another_users_memory() {
    let (_b, m, p) = setup().await;
    let (alice, bob) = (MemoryScope::User(UserId::new("alice").unwrap()), MemoryScope::User(UserId::new("bob").unwrap()));
    let mut flushed = input(&p, "alice flushed", [1.0, 0.0]);
    flushed.scope = alice.clone(); flushed.durability = WriteDurability::WaitFlush;
    m.remember(flushed).await.unwrap();
    let mut buffered = input(&p, "alice buffered", [1.0, 0.0]); buffered.scope = alice.clone();
    m.remember(buffered).await.unwrap();
    let mut q = vquery(&p, [1.0, 0.0]);
    assert!(m.recall(q.clone()).await.unwrap().hits.is_empty());               // default: Project only
    q.scopes = vec![MemoryScope::Project, bob];
    assert!(m.recall(q.clone()).await.unwrap().hits.is_empty());
    q.scopes = vec![alice];
    assert_eq!(m.recall(q).await.unwrap().hits.len(), 2);
}

#[tokio::test]   // G4
async fn session_memory_is_recalled_while_live_never_indexed_and_gone_after_ttl() {
    let b = Arc::new(InMemoryBackend::new());
    let p = ns();
    b.initialize(&p, 2, DistanceMetric::Cosine).await.unwrap();
    let m = MemoryManager::builder(b.clone(), b.clone(), b.clone())
        .with_index_interval(None).with_session_ttl(Some(Duration::from_millis(40))).build();
    let s = MemoryScope::Session(SessionId::new("s1").unwrap());
    let mut i = input(&p, "working", [1.0, 0.0]); i.scope = s.clone(); i.durability = WriteDurability::WaitFlush;
    let r = m.remember(i).await.unwrap();
    assert!(!b.has_vector(&p, r.memory_id));
    let mut q = vquery(&p, [1.0, 0.0]); q.scopes = vec![s]; q.consistency = ConsistencyMode::Eventual;
    assert_eq!(m.recall(q.clone()).await.unwrap().hits.len(), 1);
    tokio::time::sleep(Duration::from_millis(70)).await;
    assert!(m.recall(q).await.unwrap().hits.is_empty());
}
```

Add to `contract::record_store`: `session_items` returns only that session's live items, not another session's, not project-scoped items, and not expired ones.
Add to `unit_types.rs`: a `RecallQuery` with `scopes = vec![]` fails `validate()`.

- [ ] **Step 2: Run and confirm failure**

Run: `cargo test --workspace`
Expected: compile errors on `UserId`, `SessionId`, `scopes`.

- [ ] **Step 3: Implement the types, the trait method (backend, `Arc`, three middlewares, `Traced` op `record_session_items`), then the indexer and recall behaviour**

- [ ] **Step 4: Run the gate. Expected: pass**

- [ ] **Step 5: Commit**

```bash
git commit -m "feat(core): give user and session scopes an identity"
git commit -m "feat(sdk): enforce scopes in recall and keep session memory out of the indexes"
```

If the first commit cannot build without the second, make it one commit.

---

### Task 12: Middleware defaults and retry jitter

**Files:**
- Modify: `crates/aether-adapters/src/middleware.rs`, `crates/aether-sdk/src/lib.rs`
- Test: `crates/aether-adapters/tests/contract_suite.rs`, `crates/aether-sdk/tests/hardening.rs`

**Interfaces:**
- Produces: `pub fn jittered(backoff: Duration, random: u64) -> Duration` in `middleware.rs`, returning a value in `[0, backoff]` (full jitter). `retry_transient` sleeps `jittered(backoff, r)` where `r` comes from `std::collections::hash_map::RandomState::new().build_hasher().finish()`.
- Produces: `MemoryManagerBuilder::with_timeout(self, Option<Duration>) -> Self`, default `Some(Duration::from_secs(10))`.
- Produces: `MemoryManagerBuilder::with_retry(self, Option<RetryPolicy>) -> Self`, default `Some(RetryPolicy::default())`.
- `build()` wraps each store, innermost first: `Batcher` (vector store only, always), `Timeout` if set, `Resilient` if set, `Traced` if connector tracing is on. Build the stack by re-wrapping an `Arc<dyn Trait>` step by step.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn jittered_backoff_stays_within_bounds() {
    let b = Duration::from_millis(100);
    for r in [0u64, 1, 12345, u64::MAX] { assert!(jittered(b, r) <= b); }
    assert_ne!(jittered(b, 1), jittered(b, u64::MAX / 2));
}

#[tokio::test]   // hardening.rs
async fn default_manager_retries_transient_connector_failures() {
    let b = Arc::new(InMemoryBackend::new());
    let p = ns();
    b.initialize(&p, 2, DistanceMetric::Cosine).await.unwrap();
    let m = MemoryManager::builder(b.clone(), b.clone(), b.clone()).with_index_interval(None).build();
    b.fail_next(FaultPoint::VectorUpsert, 2);
    let mut i = input(&p, "a", [1.0, 0.0]); i.durability = WriteDurability::WaitFlush;
    let r = m.remember(i).await.unwrap();
    assert!(b.has_vector(&p, r.memory_id));
}
```

- [ ] **Step 2: Run and confirm failure**

Run: `cargo test --workspace`
Expected: `jittered` not found; after stubbing, `default_manager_retries_transient_connector_failures` fails with an injected fault error.

- [ ] **Step 3: Implement `jittered`, the builder options, and the wrapping order**

- [ ] **Step 4: Add `.with_retry(None)` to the `setup()` helper in `hardening.rs`**

The earlier fault tests inject a single fault and expect it to surface; the new default retry would absorb it.

- [ ] **Step 5: Run the gate. Expected: pass**

- [ ] **Step 6: Commit**

```bash
git commit -m "feat(adapters): add full jitter to retry backoff"
git commit -m "feat(sdk): apply timeout and retry middleware by default"
```

---

### Task 13: Documentation

**Files:**
- Modify: `docs/guides/memory.md`, `docs/guides/usage.md`, `docs/guides/observability.md`, `docs/architecture/index.md`, `docs/architecture/connectors.md`, `docs/architecture/rust-migration.md`, `docs/architecture/hardening-design.md`, `docs/about/roadmap.md`, `docs/index.md`

- [ ] **Step 1: Rewrite `docs/guides/memory.md`**

Cover: the three roles (record store as source of truth, two derived indexes), the scope enum with ids and the `scopes` query field, session memory being record-only with a TTL, `MemoryTier` being a filterable label with no behaviour, what `Buffered` and `WaitFlush` now guarantee, and that read-your-writes items are scored with the index metric.
Remove every mention of L1, `HotContextCache`, `IngestionBuffer`, and `Global`.

- [ ] **Step 2: Update `docs/guides/usage.md` so the quickstart compiles against the new API**

Copy the quickstart into `crates/aether-sdk/examples/quickstart.rs`, run `cargo run -p aether-sdk --example quickstart`, and paste the working code back.
Expected: prints one hit and exits 0.

- [ ] **Step 3: Update the remaining pages**

- `observability.md`: new span tree (`aether.cortex.index_pass`, `aether.cortex.index_record`, `record_*` ops).
- `architecture/index.md` and `rust-migration.md`: diagrams and crate descriptions without the cache, buffer, router, and worker pool; `RecordStore` in place of `KvStore`.
- `connectors.md`: `RecordStore` role, removal of `WalStore` and the circuit breaker, the reusable `contract` module and its feature flag, the new contract cases.
- `hardening-design.md`: status line changes to "implemented".
- `roadmap.md`: move Phase 5 into completed and update the status table.
- `docs/index.md`: link the hardening design.

- [ ] **Step 4: Build the site**

Run: `cd website && npm run build`
Expected: build succeeds with no broken links.

- [ ] **Step 5: Run the gate, then commit one page (or one tightly related pair) per commit**

```bash
git commit -m "docs(guides): describe record-store memory model and scopes"
# ...one commit per page
```

---

### Task 14: Retire the Python package

**This task deletes code and must not start without explicit sign-off from the repository owner.**

**Files:**
- Delete: `src/`, `tests/`, `pyproject.toml`, `examples/quickstart.py`
- Modify: `.github/workflows/ci.yml`, `README.md`, `examples/README.md`, `docs/development/guidelines.md`, `.gitignore`

- [ ] **Step 1: Confirm sign-off is recorded, and confirm nothing in the Rust workspace or the docs site references the Python paths**

Run: `grep -rn "aether_sdk\b\|pyproject\|src/aether_sdk" crates/ website/ docs/ .github/`
Expected: matches only in `.github/workflows/ci.yml` and `docs/development/guidelines.md`.

- [ ] **Step 2: Remove the Python job from CI; commit**

```bash
git commit -am "ci: drop python job"
```

- [ ] **Step 3: Delete the Python sources, tests, example, and `pyproject.toml`; commit**

```bash
git commit -am "chore: remove retired python package"
```

- [ ] **Step 4: Rewrite `README.md` for the Rust gateway**

Use the positioning from design section 1 and the quickstart from `docs/guides/usage.md`.
Commit.

- [ ] **Step 5: Replace `docs/development/guidelines.md` with Rust conventions**

Keep it short: the gate commands, the contract-suite requirement for new backends, error classification (`Transient` vs `Permanent`), and a pointer to `AGENTS.md`.
Run `cd website && npm run build`. Expected: success.
Commit.

---

## Spec Coverage

| Design section | Task |
|----------------|------|
| 4.1 Roles | 3, 8 |
| 4.2 `RecordStore` contract | 5, 8 (removals), 11 (`session_items`) |
| 4.3 One pipeline, indexer | 6, 7, 8 |
| 4.4 Validation | 9 |
| 4.5 Scopes, tiers, sessions | 7 (`_aether_tier`), 11 |
| 4.6 Recall | 8 (resolve through records), 10, 11 (scope check) |
| 4.7 Middleware defaults | 12 |
| 4.8 Contract suite | 4, then cases added in 5, 6, 9, 11 |
| 4.9 Repository hygiene | 1, 14 |
| G1 | 7, 8 |
| G2 | 7, 8 |
| G3 | 10 |
| G4 | 8 (deleted), 11 (scope, expiry) |
| G5 | 9 |
| G6 | 4 and every contract case |
