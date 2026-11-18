# Aether SDK Documentation

**Aether SDK** is a model-free Rust **memory SDK**: composable pieces that developers integrate into their own workflow to store, index, and retrieve AI memory across record, vector, and graph stores (**SQLite, Redis/Valkey, Turbopuffer, Neo4j, Qdrant, Chroma, and pgvector**).

## Technical Reference

- **[Architecture Overview](architecture/index.md)**: Five composable levels across the 4-crate Rust workspace.
- **[Pieces-First SDK Design](architecture/sdk-design.md)**: Design of the five levels, from single connectors to the optional `MemoryManager` facade.
- **[Phase 5 Hardening Design](architecture/hardening-design.md)**: Durable record-store source of truth with derived vector and graph indexes.
- **[Connectors Architecture](architecture/connectors.md)**: Outbound `RecordStore`, `VectorStore`, and `GraphStore` drivers, middleware decorators, and the shared contract suite.
- **[Change Overview](architecture/change-overview.md)**: Summary of the pieces-first and hardening overhaul.

## Guides

- **[Usage Guide](guides/usage.md)**: Walkthrough and runnable examples across all five SDK levels.
- **[Memory Model](guides/memory.md)**: Scopes (`Project`, `User`, `Session`), durability (`Buffered`, `WaitFlush`), consistency, and temporal provenance.
- **[Observability & OpenTelemetry](guides/observability.md)**: `tracing` span hierarchy across `aether-sdk`, `aether-cortex`, and `aether-adapters`, plus the `otel` preset.

## Development & Roadmap

- **[Development Guidelines](development/guidelines.md)**: Engineering conventions, verification gate, and contract testing.
- **[Roadmap](about/roadmap.md)**: Current status of crates, drivers, and next milestones.

---

*Built by Nytheris AI.*
