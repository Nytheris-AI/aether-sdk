# Aether SDK Documentation

**Aether SDK** is a high-performance Rust **AI Memory Gateway SDK** that enables applications to uniformly connect to AI memory storage and databases—such as **Redis/Valkey, SQLite, Turbopuffer, Neo4j, Qdrant, Chroma, and pgvector**—behind one canonical interface.

## Technical Reference

- **[Architecture Overview](architecture/index.md)**: Layered 4-crate Rust workspace and L1/L2/L3 serving model.
- **[Rust Migration Blueprint](architecture/rust-migration.md)**: Core domain contracts (`aether-core`), `aether-cortex`, and `aether-sdk`.
- **[Connectors Architecture](architecture/connectors.md)**: Outbound `KvStore`, `VectorStore`, and `GraphStore` drivers with shared resilience middleware.
- **[Phase 5 Hardening Design](architecture/hardening-design.md)**: Approved design for making the record store the source of truth, with vector and graph stores as derived indexes.

## Guides

- **[Usage Guide](guides/usage.md)**: Quickstart with `MemoryManager` (`remember`, `recall`, `update`, `forget`, `flush`, `shutdown`).
- **[Memory Model](guides/memory.md)**: `MemoryTier`, `MemoryScope`, `WriteDurability`, `ConsistencyMode`, and temporal provenance.
- **[Observability & OpenTelemetry](guides/observability.md)**: Zero-boilerplate `tracing` span hierarchy and the one-line `otel` preset.

## Development & Roadmap

- **[Development Guidelines](development/guidelines.md)**: Engineering standards, micro commits, and TDD conventions.
- **[Roadmap](about/roadmap.md)**: Real status of each component, known gaps, and what comes next.

---

*Built by Nytheris AI.*
