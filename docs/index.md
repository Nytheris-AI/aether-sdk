# Aether SDK Documentation

**Aether SDK** is a model-free Rust **memory SDK**: composable pieces that developers integrate into their own workflow to store, index, and retrieve AI memory across record, vector, and graph stores (**SQLite, Redis/Valkey, Turbopuffer, Neo4j, Qdrant, Chroma, and pgvector**).

## Technical Reference

- **[Architecture Overview](architecture/index.md)**: Five composable levels and the Obsidian vault daemon across the 5-crate Rust workspace.
- **[Pieces-First SDK Design](architecture/sdk-design.md)**: Design of the five levels, from single connectors to the optional `MemoryManager` facade.
- **[Phase 5 Hardening Design](architecture/hardening-design.md)**: Durable record-store source of truth with derived vector and graph indexes.
- **[Connectors Architecture](architecture/connectors.md)**: Outbound `RecordStore`, `VectorStore`, and `GraphStore` drivers, middleware decorators, and the shared contract suite.
- **[M1 Vault & MCP Architecture](architecture/m1-vault-mcp-rationale.md)**: Milestone 1 `nytheris-aether-vault` daemon (`VaultEngine`), single-file SQLite WAL + FTS5 + `PersonalizedPageRank` index, `Aether/Inbox.md` reconciliation, and `aether-mcp` stdio server.
- **[Change Overview](architecture/change-overview.md)**: Summary of the pieces-first and hardening overhaul.

## Guides

- **[Usage Guide](guides/usage.md)**: Walkthrough and runnable examples across all five SDK levels plus `nytheris-aether-vault` and `aether-mcp`.
- **[Memory Model](guides/memory.md)**: Core SDK scopes (`Project`, `User`, `Session`), durability (`Buffered`, `WaitFlush`), consistency, temporal provenance, and the Obsidian vault AMU model.
- **[Observability & OpenTelemetry](guides/observability.md)**: `tracing` span hierarchy across `nytheris-aether-sdk`, `nytheris-aether-cortex`, and `nytheris-aether-adapters`, plus the `otel` preset.

## Development & Roadmap

- **[Development Guidelines](development/guidelines.md)**: Engineering conventions, verification gate, and contract testing.
- **[Roadmap](about/roadmap.md)**: Current status of crates, drivers, and next milestones.

---

*Built by Nytheris AI.*
