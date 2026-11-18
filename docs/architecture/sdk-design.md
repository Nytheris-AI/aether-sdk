# Aether SDK Design - Pieces First

Status: **implemented** (2026-11-18).
This document sets the shape of Aether.
The [change overview](./change-overview.md) lists every removal, change, and addition in one place.
It supersedes the "gateway" framing everywhere else in these docs.
The consistency mechanics in the [Phase 5 hardening design](./hardening-design.md) stay valid and become one of the pieces described here; section 7 lists exactly what this document changes in it.

## 1. Intent

Aether is an **SDK that developers integrate into their own workflow**.
It is a set of small pieces a developer calls from their own code, in their own order, next to their own steps.
It is not a gateway that the workflow has to pass through.

The audience is unchanged: developers who run their own memory, RAG, or knowledge-base stack.
The headline value is unchanged: keeping a record, a vector, and graph facts consistent for one memory.
What changes is how that value is delivered.
Before, there was one door (`MemoryManager`) and the developer accepted its whole pipeline.
Now each capability is reachable on its own, and the one-door facade is a convenience built from the same public pieces.

Aether stays model-free.
It does not embed, chunk, extract, or call models.
Pieces-first is what makes that comfortable: the developer's workflow does the embedding, and calls Aether on either side of it.

## 2. Goals, Non-Goals, Anti-Goals

**Goals**

- S1. Every capability is usable without the facade.
- S2. A developer can adopt one piece (for example, only a vector connector with retries) without adopting the rest.
- S3. Only the stores a developer uses are required.
- S4. Nothing runs in the background unless the developer starts it.
- S5. A developer can put their own step between any two Aether steps without forking Aether.
- S6. The guarantees from the hardening design (G1 to G6) still hold, and each one is attributed to the piece that provides it.

**Non-goals**

- A plugin or hook system.
- A server, a wire protocol, or an MCP endpoint.
- Model calls of any kind.

**Anti-goals**

- A facade that can do something the public pieces cannot.
- A piece that needs a shared context object, global state, or a running runtime task to work.
- A second, "advanced" API that drifts from the simple one.

## 3. Approaches Considered

**A. Keep the single facade and add hooks.**
`MemoryManager` stays the only entry point and gains callbacks (before write, after search, rerank).
It keeps one simple API.
It is still inversion of control: Aether calls the developer's code, the developer cannot reorder steps, and every new need becomes a new hook.
This is a framework, which is the shape being moved away from.

**B. Pieces first, facade second (chosen).**
The capabilities are public functions and small structs that take what they need as arguments.
The facade is a thin composition of them and lives in a crate that can only see their public API.
The developer's own code is the pipeline.
Cost: the public surface is larger and each piece needs its own documentation and stability.

**C. One crate per piece.**
The same as B, split into many small crates.
It gives finer dependency control, and adds versioning and release overhead that feature flags on four crates already avoid.

B was chosen because it is the only option where S1 and S5 are true by construction.

## 4. The Pieces

### 4.1 Levels of use

A developer can stop at any level.
Each level is built only from the ones above it.

| Level | Pieces | What you get | Crate |
|-------|--------|--------------|-------|
| 1. Connectors | `RecordStore`, `VectorStore`, `GraphStore` and their drivers; `Traced`, `Resilient`, `Timeout`, `Batcher` | One portable client per store kind, with tenant isolation, portable filters, retries, and tracing | `aether-core`, `aether-adapters` |
| 2. Functions | `DistanceMetric::score`, `FilterExpr::matches`, `rrf`, `PersonalizedPageRank`, `graph_scores` | Scoring, filtering, and fusion over your own data, no I/O | `aether-core`, `aether-cortex` |
| 3. Sync engine | `Indexer` over a `RecordStore` | Multi-store consistency: nothing lost, nothing resurrected, retried until indexed | `aether-cortex` |
| 4. Recall steps | `recall::vector_search`, `recall::unindexed`, `recall::session`, `recall::graph_search`, `recall::merge`, `recall::in_scopes` | Each retrieval step on its own, composable with your steps | `aether-cortex` |
| 5. Facade | `MemoryManager` | All of the above in `remember`, `recall`, `update`, `forget` | `aether-sdk` |

### 4.2 Rules every piece follows

1. **Arguments, not context.**
   A piece takes the stores and values it needs as parameters (`&dyn VectorStore`, `&ProjectId`).
   There is no shared context object.
2. **No hidden work.**
   No piece spawns a task, starts a timer, or keeps global state.
   Background indexing exists only as an explicit helper the developer calls.
3. **Facade has no private powers.**
   `aether-sdk` depends on `aether-cortex` and `aether-adapters` as an outside crate would.
   If the facade needs something, it must be public, so developers have it too.
4. **Guarantees are attributed.**
   Each piece's documentation states which guarantee it provides and what the caller must do to keep it.
5. **Plain data between steps.**
   Steps exchange ordinary types (`Vec<Candidate>`, `MemoryItem`), so a developer's own step fits between any two.

### 4.3 The write path as pieces

```rust
// The developer's workflow owns the order and the timing.
let embedding = my_embedder.embed(&text).await?;          // theirs
let mut input = MemoryInput::new(ns.clone(), text);
input.embedding = Some(embedding);
input.validate()?;                                         // piece: validation
spec.check(input.embedding.as_deref())?;                   // piece: dimension check

let item = MemoryItem::from_input(input, 1);
records.create(&ns, &item, None).await?;                   // piece: durable record (G1)

indexer.run_pass().await?;                                 // piece: sync engine, when they choose
```

`Indexer` is constructed with only the indexes in use:

```rust
let indexer = Indexer::new(records.clone()).with_vector(vectors.clone());   // no graph store
```

With no vector store it skips vector writes, and likewise for the graph.
`Indexer::spawn(self: &Arc<Self>, interval: Duration) -> JoinHandle<()>` is the opt-in background loop.

### 4.4 The recall path as pieces

```rust
pub struct Candidate { pub item: MemoryItem, pub score: f32 }
```

```rust
let qvec = my_embedder.embed(&question).await?;                                        // theirs

let indexed = recall::vector_search(&*vectors, &*records, &ns, &qvec, 50, None).await?; // search + resolve (G2, G4)
let fresh   = recall::unindexed(&*records, &ns, &qvec, spec.metric, None).await?;       // read-your-writes (G3)
let mut candidates = recall::merge(vec![indexed, fresh]);

candidates = my_reranker.rerank(&question, candidates).await?;                          // theirs, in the middle
candidates = recall::in_scopes(candidates, &[MemoryScope::Project]);                    // scope check (G4)
```

| Step | Signature (abridged) | Guarantee it carries |
|------|----------------------|----------------------|
| `vector_search` | `(vectors, records, ns, query_vector, top_k, filter) -> Vec<Candidate>` | Resolves every hit through the record store, so deleted and expired memories are dropped |
| `unindexed` | `(records, ns, query_vector, metric, filter) -> Vec<Candidate>` | Read-your-writes with the index's own metric |
| `session` | `(records, ns, session, query_vector, metric, filter) -> Vec<Candidate>` | Live session memory only |
| `graph_search` | `(graph, records, ns, seeds, hops, per_node_limit, temporal, ppr) -> (Vec<Candidate>, Subgraph)` | Graph-ranked memories, resolved through records |
| `merge` | `(Vec<Vec<Candidate>>) -> Vec<Candidate>` | Same-metric lists, de-duplicated by id keeping the newest version, sorted |
| `rrf` | `(&[Vec<Candidate>], k) -> Vec<Candidate>` | Rank fusion across different metrics |
| `in_scopes` | `(Vec<Candidate>, &[MemoryScope]) -> Vec<Candidate>` | Drops anything outside the requested scopes |
| `scope_filter` | `(&[MemoryScope]) -> Option<FilterExpr>` | The index-side scope filter to pass to `vector_search` |

The ranking algorithm is the one in hardening design section 4.6.
It is now expressed as these steps instead of one `QueryPlanner`, and `QueryPlanner` is removed.

### 4.5 The facade

`MemoryManager` keeps its name and its four operations.
It changes in these ways:

- **Built from public pieces only** (rule 3).
  Its `recall` is roughly thirty lines composing section 4.4, and the usage guide shows that source as the template for writing your own.
- **Stores are optional.**
  `MemoryManager::builder(records)` takes the record store.
  `.vector(v)` and `.graph(g)` add indexes.
  A recall mode that needs an absent store returns `InvalidInput`.
- **A record store is always required by the facade**, because it is where content lives.
  `InMemoryBackend` and (Phase 6) SQLite make that zero-infrastructure.
  A developer who wants only a vector client uses level 1 directly.
- **No background work by default.**
  `.background_indexing(interval)` opts in.
  Without it, a `Buffered` write is recorded durably, is visible to read-your-writes recall, and is indexed on the next `flush()`.
- **`MemoryInbound` is removed.**
  It existed so HTTP, gRPC, and MCP transports could share one object-safe trait.
  Those transports are no longer planned, and a trait with one implementation is not kept.
  The operations become inherent methods.
- **Middleware defaults stay in the builder** (`Timeout`, `Resilient`, `Traced`), each switchable.
  A developer composing pieces by hand wraps their own stores.

### 4.6 `InMemoryBackend` is a real piece

It is currently in a module named `memory_test` and described as test-only.
It is the zero-infrastructure backend for local development and the reference implementation of all three store contracts.
The module is renamed `in_memory`.
The fault-injection methods stay, documented as test aids.

## 5. Where Each Guarantee Lives

| Guarantee | Provided by | What a developer composing by hand must do |
|-----------|-------------|--------------------------------------------|
| G1 no lost write | `RecordStore::create` + `Indexer` | Write the record first; call `run_pass` (or `spawn`) |
| G2 no resurrection, no orphans | `RecordStore::tombstone` + `Indexer`; resolve inside every recall step | Delete through `tombstone`, never directly in an index |
| G3 honest read-your-writes scores | `recall::unindexed` | Pass the index's metric from `VectorStore::describe` |
| G4 no expired or out-of-scope results | Resolve inside recall steps; `scope_filter`; `in_scopes` | Apply `in_scopes` last |
| G5 bad input fails early | `MemoryInput::validate`, `RecallQuery::validate`, `IndexSpec::check` | Call them before writing |
| G6 any backend passes the same tests | `aether_adapters::contract` | Run it against a custom driver |

The facade does every item in the right-hand column for the developer.
That is its whole reason to exist.

## 6. What Is Removed From the Roadmap

- HTTP/gRPC gateway server.
- MCP server.
- The `MemoryInbound` transport abstraction.

Language bindings (for example Python through `PyO3`) are not decided here.
A pieces-first Rust API does not translate one-to-one into another language, so that is its own design when the need is real.

## 7. Changes to the Hardening Design

| Hardening design section | Change |
|--------------------------|--------|
| 1 Intent | "Storage gateway" becomes "model-free SDK". The problem statement stays as the description of the sync engine piece. |
| 4.2 `RecordStore` contract | Unchanged. |
| 4.3 Pipeline and indexer | `Indexer` takes optional vector and graph stores. The background loop is opt-in through `Indexer::spawn`. |
| 4.4 Validation | Unchanged rules, exposed as callable pieces (`IndexSpec::check` added). |
| 4.5 Scopes and sessions | Unchanged rules, exposed as `scope_filter` and `in_scopes`. |
| 4.6 Recall | Same algorithm, delivered as the public steps in section 4.4 here. `QueryPlanner` is removed. |
| 4.7 Middleware defaults | Apply in the facade builder only. |
| 4.8 Contract suite | Unchanged. |

## 8. Effect on the Implementation Plan

The earlier gateway-hardening plan was written before this document.

- Tasks 1 to 7 (CI, shared functions, rename, contract suite, versioned records, idempotent relations, `Indexer`) are unaffected, apart from `Indexer` taking optional stores.
- Tasks 8 to 13 (facade switch, validation, recall, scopes, middleware, docs) change shape and must be revised against this document before anyone executes them.
- Task 14 (Python removal) is unaffected and still needs sign-off.

## 9. Production Impact

None on running systems: nothing is released and no production driver exists.
The public Rust API changes in breaking ways on top of those already listed in the hardening design: `MemoryInbound` removed, `MemoryManager::new` replaced by a builder taking the record store, `QueryPlanner` removed, module `memory_test` renamed.

## 10. Calls Made Here (not yet confirmed)

- No hook or plugin system; composition is the extension mechanism.
- Background indexing is off by default.
- The facade always requires a record store; vector and graph are optional.
- `MemoryInbound` is deleted, not kept for future transports.
- Servers and MCP leave the roadmap; language bindings are deferred, not rejected.
- Crate names and the `MemoryManager` name stay as they are.
- `memory_test` is renamed `in_memory`.
- The empty driver feature flags in `aether-adapters` are removed until each driver exists.
- The planned `config.rs` and `ConnectorRegistry` are dropped: developers construct the stores they use.
- `docs/architecture/rust-migration.md` is deleted once the work lands; the two design docs and rustdoc replace it.

## 11. Left to the Builder

- Exact parameter order and borrowing style of the recall steps, kept consistent across all of them.
- Whether `Candidate` carries extra fields (for example which step produced it) once a real workflow needs them.
- How the usage guide presents the levels; one runnable example per level is the minimum.
