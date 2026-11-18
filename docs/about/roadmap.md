# Aether SDK Roadmap

Aether is a model-free Rust **memory SDK**: composable pieces developers call from their own workflow to store, index, and retrieve AI memory across **SQLite, Redis/Valkey, Turbopuffer, Qdrant, Chroma, pgvector, and Neo4j**.
Architecture decisions live in the [pieces-first SDK design](../architecture/sdk-design.md) and [Phase 5 hardening design](../architecture/hardening-design.md).

## Current Status

| Area | Status | Notes |
|------|--------|-------|
| `nytheris-aether-core` contracts | Done | Newtypes (`ProjectId`, `MemoryId`, `EntityId`, `UserId`, `SessionId`), `RecordStore` / `VectorStore` / `GraphStore`, `IndexSpec`, shared scoring & filters, `AetherError`. |
| `nytheris-aether-adapters::in_memory` + `contract-suite` | Done | Thread-safe `InMemoryBackend` with `FaultPoint` injection + reusable `contract::{record_store, record_store_concurrent_idempotency, vector_store, graph_store}`. |
| Middleware (`Batcher`, `Timeout`, `Resilient`, `Traced`) | Done | `Batcher` -> `Timeout` -> `Resilient` (exponential backoff + full jitter via `jittered`) -> `Traced`. Applied by default in `MemoryManager::builder`. |
| `nytheris-aether-cortex` (`Indexer`, `recall`, `ppr`) | Done | Durable `RecordStore`-driven sync engine (`run_pass`, `apply`, opt-in `spawn`), composable recall steps, `PersonalizedPageRank` + `graph_scores`, `RRF` fusion. |
| `MemoryManager` facade (`nytheris-aether-sdk`) | Done | Optional builder accepting `RecordStore` + optional `.vector(...)`, `.graph(...)`, `.background_indexing(...)`, middleware toggles. |
| Production drivers (`sqlite`, `redis`, `turbopuffer`, `qdrant`, `chroma`, `pgvector`, `neo4j`) | Done | Implemented behind feature flags and verified via contract unit tests + live local Docker verification (`Valkey`, `Qdrant`, `Chroma`, `pgvector`, `Neo4j`) + `SQLite`. |
| Observability & Rust CI | Done | End-to-end `tracing` spans + `otel` OTLP preset; GitHub Actions checks `cargo fmt`, `clippy -D warnings`, `cargo test --workspace --all-features`, and docs build. |

## Completed Milestones

### Phase 5 - Pieces-First SDK & Consistency Hardening (Completed 2026-11-18)

- Replaced in-process `IngestionBuffer`, `HotContextCache`, `PolicyRouter`, and `QueryPlanner` with `RecordStore` versioned durability (`create`, `get_many`, `cas_update`, `tombstone`, `pending`, `pending_for`, `session_items`, `mark_indexed`) and `nytheris_aether_cortex::{Indexer, recall}`.
- Removed `MemoryInbound` in favor of inherent methods on `MemoryManager` assembled solely from public pieces.
- Enforced `VectorStore::describe` / `IndexSpec` validation at ingest/update/query boundaries and required matching embeddings when updating content.
- Added identity-carrying `MemoryScope::{Project, User(UserId), Session(SessionId)}` with record-only TTL-bound session storage.
- Wired `PersonalizedPageRank` + `graph_scores` into `RecallMode::Hybrid` via Reciprocal Rank Fusion (`recall::rrf`) and added `jittered` exponential backoff + `Timeout` + `Resilient` defaults on `MemoryManager::builder`.

### Phase 6 - Production Backends & Live Verification (Completed 2026-11-18)

- Implemented and upgraded all 7 backend drivers (`sqlite`, `redis`, `turbopuffer`, `qdrant`, `chroma`, `pgvector`, `neo4j`) to Phase 5 contracts including persistent cross-restart `describe` resolution on `qdrant`, `chroma`, and `pgvector`.
- Verified `sqlite` + mocked suites (`turbopuffer`, `qdrant`, `chroma`, `neo4j`, `pgvector`) offline in CI, plus live Docker integration and end-to-end `MemoryManager` suites against Valkey, Qdrant, Chroma, pgvector, and Neo4j.

## Scope & Next Directions

- **Language Bindings (deferred)**: Foreign-language bindings (for example Python via `PyO3`) remain deferred until demanded by concrete downstream users.
- **Model-free invariant**: Aether does not call embedders, chunkers, or LLMs; callers bring their own vectors and extracted entities/relations.
