# Aether SDK Architecture

Aether SDK is a hexagonal, protocol-driven **AI Memory Gateway** in Rust. It decouples agent memory orchestration from specific storage and database engines so you can swap or combine **Redis/Valkey, SQLite, Turbopuffer, Neo4j, Qdrant, Chroma, and pgvector** without changing application logic.

:::note Shape is changing
This page describes the code as it is today, where `MemoryManager` is the single entry point.
The [pieces-first SDK design](./sdk-design.md) makes every component below usable on its own and turns `MemoryManager` into an optional facade.
:::

## High-Level Architecture

```mermaid
graph TD
    subgraph Inbound ["1. Inbound (aether-sdk)"]
        MM[MemoryManager / MemoryInbound]
    end

    subgraph Core ["2. Gateway Core (aether-cortex)"]
        PR[PolicyRouter]
        HC["L1: HotContextCache"]
        IB["IngestionBuffer (Write-Behind)"]
        QP["QueryPlanner + PPR (petgraph)"]
    end

    subgraph Outbound ["3. Outbound (aether-adapters)"]
        MW["Middleware: Traced<Resilient<Timeout<Batcher>>>"]
        KV["L2 KvStore (Redis/Valkey, SQLite)"]
        VEC["L3 VectorStore (Turbopuffer, Qdrant, Chroma, pgvector)"]
        GRP["L3 GraphStore (Neo4j — provenance & temporal)"]
    end

    MM --> PR & QP
    PR --> HC & KV & IB
    IB --> MW
    QP --> HC & KV & IB & MW
    MW --> KV & VEC & GRP
```

## Workspace Crates

1. **`aether-core`**: Pure domain contracts (`ProjectId`, `MemoryId`, `EntityId`, `FilterExpr`, `TemporalFilter`, `MemoryInbound`, `OutboundConnector`, `KvStore`, `VectorStore`, `GraphStore`).
2. **`aether-cortex`**: Orchestration core (`PolicyRouter`, `HotContextCache`, `IngestionBuffer`, `PersonalizedPageRank`, `QueryPlanner`).
3. **`aether-adapters`**: Shared outbound middleware (`Traced`, `Resilient`, `Timeout`, `Batcher`), `InMemoryBackend` for tests/dry-runs, and feature-gated production storage drivers.
4. **`aether-sdk`**: Public non-generic `MemoryManager` facade with CAS updates, `ReadYourWrites` consistency, and graceful `flush()` / `shutdown()`.
