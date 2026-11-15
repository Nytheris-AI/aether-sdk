# Aether SDK Documentation

**Aether SDK** is a high-performance Rust **AI Memory Gateway SDK** that enables applications to uniformly connect to AI memory storage and databases—such as **Redis/Valkey, SQLite, Turbopuffer, Neo4j, Qdrant, Chroma, and pgvector**—behind one canonical interface.

## Technical Reference

- **[Architecture Overview](architecture/index.md)**: Layered 4-crate Rust workspace and L1/L2/L3 serving model.
- **[Rust Migration Blueprint](architecture/rust-migration.md)**: Core domain contracts (`aether-core`), `aether-cortex`, and `aether-sdk`.
- **[Connectors Architecture](architecture/connectors.md)**: Outbound `KvStore`, `VectorStore`, and `GraphStore` drivers with shared resilience middleware.

## Guides

- **[Usage Guide](guides/usage.md)**: Quickstart with `MemoryManager` (`remember`, `recall`, `update`, `forget`, `flush`, `shutdown`).
- **[Memory Model](guides/memory.md)**: `MemoryTier`, `MemoryScope`, `WriteDurability`, `ConsistencyMode`, and temporal provenance.

## Development & Roadmap

- **[Development Guidelines](development/guidelines.md)**: Engineering standards, micro commits, and TDD conventions.
- **[Roadmap](about/roadmap.md)**: Current milestones and upcoming storage/database drivers.

---

*Built by Nytheris AI.*
