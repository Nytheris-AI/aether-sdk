# Aether SDK Roadmap

Aether SDK is built as a unified Rust **AI Memory Gateway** for connecting applications to AI memory storage and databases (**Redis/Valkey, SQLite, Turbopuffer, Neo4j, Qdrant, Chroma, pgvector**).

## Completed Milestones

- **Phase 1 — `aether-core`**: Strongly typed newtypes (`ProjectId`, `MemoryId`, `EntityId`), `FilterExpr` AST, `TemporalFilter`, `AetherError` (`Transient` vs `Permanent`), and object-safe `MemoryInbound`, `OutboundConnector`, `KvStore`, `VectorStore`, and `GraphStore` traits.
- **Phase 1.5 — `aether-adapters` Foundation**: `InMemoryBackend` (`KvStore` + `VectorStore` + `petgraph` `GraphStore`), shared middleware (`Traced`, `Resilient`, `Timeout`, `Batcher`), and the contract test suite.
- **Phase 2 — `aether-cortex`**: `PolicyRouter`, L1 `HotContextCache`, `IngestionBuffer`, `PersonalizedPageRank`, and `QueryPlanner` with `ReadYourWrites` L1/L2/L3 merging.
- **Phase 4 — `aether-sdk`**: Non-generic `MemoryManager` facade with atomic idempotency, `Session` TTLs, CAS `update()`, provenance-safe `forget()`, and `flush()` / `shutdown()`.

## Upcoming Milestones

- **L2 Production Drivers (`aether-adapters/src/kv/`)**:
  - `sqlite` (WAL-mode embedded file database)
  - `redis` (multiplexed async Redis/Valkey `ConnectionManager`)
- **L3 Production Drivers (`aether-adapters/src/storage/`)**:
  - `turbopuffer`, `qdrant`, `chroma`, `pgvector` (`VectorStore`)
  - `neo4j` (`GraphStore` with `source_memory_id` provenance and temporal filtering)
- **Language & Service Bindings**: Optional `PyO3` Python bindings and HTTP/gRPC gateway server.
