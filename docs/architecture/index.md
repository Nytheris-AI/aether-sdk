# Aether SDK Architecture

Aether SDK is a model-free Rust **memory SDK** for storing, indexing, and retrieving AI memory across durable **record, vector, and graph stores** (**SQLite, Redis/Valkey, Turbopuffer, Qdrant, Chroma, pgvector, and Neo4j**). Every layer is exposed as standalone composable pieces, with `MemoryManager` serving as an optional convenience facade.

## Five Composable Levels

1. **Level 1 - Single Connectors (`muninn-aether-core` + `muninn-aether-adapters`)**: Call `RecordStore`, `VectorStore`, or `GraphStore` directly (or `InMemoryBackend` for local runs).
2. **Level 2 - Middleware (`muninn-aether-adapters`)**: Stack `Batcher`, `Timeout`, `Resilient` (exponential backoff with full jitter on transient errors), and `Traced` around any connector.
3. **Level 3 - Indexing & Recall Primitives (`muninn-aether-cortex`)**: Combine `recall::{scope_filter, vector_search, unindexed, session, graph_search, merge, rrf, in_scopes}` and `PersonalizedPageRank` / `graph_scores` alongside your own application logic.
4. **Level 4 - Sync Engine (`muninn-aether-cortex::Indexer`)**: Derive vector and graph indexes asynchronously or on demand from versioned `RecordStore` pending items and tombstones (`run_pass`, `apply`, opt-in `spawn`).
5. **Level 5 - Optional Facade (`muninn-aether-sdk::MemoryManager`)**: Configure `MemoryManager::builder(record_store)` with optional `.vector(...)`, `.graph(...)`, `.background_indexing(...)`, and middleware toggles to run `remember`, `recall`, `update`, `forget`, `flush`, and `shutdown`.

## Workspace Crates

1. **`muninn-aether-core`**: Domain contracts, validated newtypes (`ProjectId`, `MemoryId`, `EntityId`, `UserId`, `SessionId`), shared scoring (`DistanceMetric::score`) and filtering (`FilterExpr::matches`), `IndexSpec`, `AetherError`, and storage traits (`OutboundConnector`, `RecordStore`, `VectorStore`, `GraphStore`).
2. **`muninn-aether-cortex`**: Composable `Indexer`, `PersonalizedPageRank` + `graph_scores`, and `recall` steps.
3. **`muninn-aether-adapters`**: `InMemoryBackend` (with `FaultPoint` injection), middleware decorators (`Batcher`, `Timeout`, `Resilient`, `Traced`), reusable `contract` test suite (`contract-suite` feature), and feature-gated backend drivers (`sqlite`, `redis`, `turbopuffer`, `qdrant`, `chroma`, `pgvector`, `neo4j`).
4. **`muninn-aether-sdk`**: `MemoryManager` / `MemoryManagerBuilder` composition root and optional OpenTelemetry preset (`otel` feature).
5. **`muninn-aether-vault`**: Local Obsidian vault daemon (`VaultEngine`), single-file SQLite WAL + FTS5 + `PersonalizedPageRank` (`muninn_aether_cortex`) + `CrossEncoderReranker`, pluggable `Embedder` (`HashedNgramEmbedder`, `DenseSubwordEmbedder`, `LocalGpuEmbedder`) with `vault_meta` auto-reindexing, deterministic `HOT` prompt prefix caching (`static_prefix` / `dynamic_context`), `Aether/Inbox.md` staging + `Aether/Compiled/Approved.md` mirror, Daily Note Dream worker, loopback `ControlPlaneServer`, and 3-tool `stdio` MCP server (`McpServer`, `aether-mcp` binary). See [Vault & MCP Architecture](./m1-vault-mcp-rationale.md).
