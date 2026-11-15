# Memory Model & Consistency

Aether organizes agent memory across **tiers**, **scopes**, and **three storage layers (L1 / L2 / L3)** so short-term working memory and long-term semantic/graph knowledge behave predictably in production.

## Memory Tiers (`MemoryTier`)
- **`Episodic`**: Time-bound events and interaction history.
- **`Semantic`** *(default)*: Durable facts and structured knowledge.
- **`Procedural`**: Workflows, tool-use patterns, and learned instructions.
- **`Profile`**: User or agent preferences and long-lived attributes.

## Memory Scopes (`MemoryScope`)
- **`Project`** *(default)*: Shared within a `ProjectId` namespace.
- **`User`**: Scoped to an end-user within a project.
- **`Session`**: Short-term working memory stored in L2 `KvStore` with automatic TTL expiration.
- **`Global`**: Cross-session shared knowledge within a project.

## Durability & Consistency

### `WriteDurability`
- **`Buffered`** *(default)*: Enqueues writes to `IngestionBuffer` after L2 `KvStore` + L1 `HotContextCache` persistence and returns `ReceiptState::Buffered` immediately.
- **`WaitFlush`**: Awaits the flush barrier to L3 (`VectorStore` + `GraphStore`) and returns `ReceiptState::Persisted`.

### `ConsistencyMode`
- **`ReadYourWrites`** *(default)*: `QueryPlanner` merges un-flushed `IngestionBuffer` entries, L1 `HotContextCache`, and L2 `KvStore::recent` with L3 `VectorStore` / `GraphStore` results.
- **`Eventual`**: Queries L3 outbound stores directly for lowest-overhead reads when immediate write visibility is not required.
