# Hybrid Recall, Personalized PageRank & RRF

Pure vector similarity misses relational hops across entities, while pure graph traversal misses lexical and semantic paraphrase. This page explains how `muninn-aether-core`, `muninn-aether-cortex`, and `muninn-aether-vault` score vectors, evaluate portable filters, traverse subgraphs with `PersonalizedPageRank`, and fuse rankings deterministically with Reciprocal Rank Fusion.

## Vector scoring (`DistanceMetric::score`)

Every vector backend and in-process scorer shares one scoring contract in `muninn_aether_core::DistanceMetric::score`:

| Metric | Formula | Zero-norm or sign rule |
| --- | --- | --- |
| `DistanceMetric::Cosine` (default) | `dot(a, b) / (norm(a) * norm(b))` | Returns `0.0` when either vector has zero norm |
| `DistanceMetric::Dot` | `sum(a_i * b_i)` | Higher dot product ranks higher |
| `DistanceMetric::Euclidean` | `-sqrt(sum((a_i - b_i)^2))` | Negated distance so closer vectors always have higher scores |

Negating Euclidean distance is deliberate. It ensures that `recall::merge` can combine `VectorStore::search` results with `recall::unindexed` and `recall::session` results using descending score order regardless of which metric the collection uses.

## Portable filtering (`FilterExpr`)

`FilterExpr` defines a backend-agnostic filter AST translated by `QdrantVectorStore`, `PgVectorStore`, `ChromaVectorStore`, and `TurbopufferVectorStore` and evaluated in memory by `FilterExpr::matches` for `InMemoryBackend`, `recall::unindexed`, and `recall::session`.

- `FilterExpr::Eq { field, value }` matches `String`, `Int` (`i64`), `Float` (`f64` within `f64::EPSILON`), or `Bool`.
- `FilterExpr::In { field, values }` matches any element in `values`.
- `FilterExpr::Range { field, gte, lte }` evaluates numeric bounds (`>= gte` and `<= lte`).
- `FilterExpr::And(exprs)` and `FilterExpr::Or(exprs)` compose nested predicates.

`MemoryManager` builds `recall::scope_filter(&query.scopes)` on `_aether_scope` and wraps it with `query.filters` using `FilterExpr::And` so user queries stay isolated inside their allowed tenant scopes.

## Sparse subgraph extraction (`graph_search`)

Dense graphs cannot be loaded into process memory on every query. `recall::graph_search` calls `GraphStore::neighborhood(ns, seeds, hops, per_node_limit, temporal)` to extract only a bounded `Subgraph` (`entities` and `edges`).

In `MemoryManager`, `RecallMode::Graph` uses `query.seed_entities` directly. In `RecallMode::Hybrid`, `resolve_hybrid_seeds` combines `query.seed_entities` with every entity ID (`Entity::id`) and relation endpoint (`Relation::source`, `Relation::target`) found on the resolved `MemoryItem` hits from `vector_recall_branch`. That design lets hybrid queries discover graph paths even when the caller supplies only a query vector and no explicit `seed_entities`.

## Iterative `PersonalizedPageRank` and `graph_scores`

Once `Subgraph` is in memory, `PersonalizedPageRank::rank` builds a `petgraph::DiGraph<EntityId, ()>` and runs power iteration (`damping = 0.85`, `iterations = 20` by default).

- Seeds present in the subgraph receive equal personalization mass `1.0 / seed_hits`. If none of `seeds` exist in the subgraph, personalization falls back to uniform `1.0 / n`.
- Each iteration pushes `damping * score / out_deg` along outgoing edges, redistributes dangling node mass (`out_deg == 0`) along the personalization vector, and adds `(1.0 - damping) * personalization`.
- `graph_scores(&subgraph, &entity_ranks)` projects entity importance back onto memories by adding `rank(edge.source) + rank(edge.target)` to `edge.source_memory_id` for every edge in the subgraph.
- `recall::graph_search` resolves those `MemoryId` keys through `RecordStore::get_many`, dropping any deleted or missing records.

## Reciprocal Rank Fusion (`recall::rrf`)

Cosine similarity scores and PageRank weights sit on incompatible numerical scales. Normalizing raw scores across backends is brittle when score distributions shift per query.

Instead, `recall::rrf(lists, RRF_K)` uses rank positions (`rank = idx + 1`) with smoothing constant `RRF_K = 60.0`:

```text
rrf_score(id) = sum over lists containing id of (1.0 / (RRF_K + rank))
```

When two lists contain different versions of the same `MemoryId`, `rrf` keeps the `MemoryItem` with the higher `version` and sorts descending by fused score (breaking ties deterministically by `MemoryId`).

## Vault extensions (`authority_weight` and `CrossEncoderReranker`)

`muninn-aether-vault` adapts the same `PersonalizedPageRank`, `graph_scores`, and `recall::rrf` primitives over local Obsidian notes and adds three vault-specific signals:

1. **Three-way RRF.** `VaultStore::recall_bundle` gathers up to `32` FTS5 BM25 hits (`amus_fts` with `porter unicode61` tokenizer), up to `32` cosine vector hits (from `HashedNgramEmbedder`, `DenseSubwordEmbedder`, or `LocalGpuEmbedder`), and graph neighbors over `[[wikilinks]]` and `#tags`, then fuses all three lists via `recall::rrf`.
2. **Authority weighting.** Each RRF score is multiplied by the AMU's `authority_weight` (`1.0` for `Profile`, `Rule`, `Decision`, and `Spec`, `0.6` for `Fleeting`, and `0.3` for sections where unchecked `- [ ]` task lines outnumber checked tasks plus prose lines).
3. **Cross-encoder reranking.** `CrossEncoderReranker` evaluates the top `DEFAULT_RERANK_TOP_K` (`30`) entries in place without network calls, adding a bonus over four signals (`0.40 * token_coverage + 0.30 * ordered_bigram_ratio + 0.15 * phrase_bonus + 0.15 * heading_coverage`) scaled by `authority_weight.max(0.1) * 0.10`. Callers can substitute `NoopReranker` or a custom `Reranker` on `VaultConfig::with_reranker`.
