# Changelog

All notable changes to **Aether SDK** are documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- `aether-core`: Pure domain contracts, validated newtypes (`ProjectId`, `MemoryId`, `EntityId`, `UserId`, `SessionId`), `DistanceMetric::score`, `FilterExpr::matches`, `IndexSpec`, `SecretString`, `redact_url_userinfo`, `AetherError`, and `OutboundConnector`, `RecordStore`, `VectorStore`, and `GraphStore` traits.
- `aether-cortex`: Durable `RecordStore`-driven `Indexer` sync engine (`run_pass`, `apply`, opt-in `spawn`), `PersonalizedPageRank` + `graph_scores`, and `recall` primitives (`vector_search`, `graph_search`, `unindexed`, `session`, `merge`, `rrf`).
- `aether-adapters`: `InMemoryBackend` with `FaultPoint` injection, `Batcher`, `Timeout`, `Resilient` (exponential backoff with full `jittered` delay), `Traced`, reusable `contract` test suite (`contract-suite`), and feature-gated backend drivers (`sqlite`, `redis`, `qdrant`, `pgvector`, `chroma`, `turbopuffer`, `neo4j`).
- `aether-sdk`: Optional `MemoryManager` and `MemoryManagerBuilder` facade, runnable examples (`level_1_connector` through `level_5_facade`, `otel_demo`), and optional OpenTelemetry OTLP preset (`otel` feature).
- Documentation portal built with Docusaurus in `website/`, local Docker dev stack in `infra/`, and CI gates for Rust (`rustfmt`, `clippy`, tests), website build, and `gitleaks` secret scanning.
