# Connectors Architecture (Unified Storage & DB Layer)

At the core of `aether-sdk` is its unified outbound connector layer (`aether-adapters`), which lets applications connect uniformly to AI memory storage and databases—such as **Turbopuffer, Neo4j, Qdrant, Chroma, and pgvector**—behind one consistent API. Connectors are structured so every downstream driver inherits resilience, batching, tenant isolation, and observability without duplicating boilerplate.

---

## 1. Multi-Tier Storage & Database Connector Traits

Aether separates fast working-memory state from durable semantic/graph storage using an **L1 → L2 → L3** serving model:

- **L1 (`HotContextCache` in `aether-cortex`)**: Process-local bounded cache (`moka` / ring buffer) shielding L2/L3 from hot-key bursts.
- **L2 (`KvStore: OutboundConnector`)**: Short-term / session memory, TTL expiration, atomic idempotency (`put_if_absent`), distributed CAS (`cas_update`), and cross-replica Read-Your-Writes backed by **Redis/Valkey** (multiplexed async TCP) or **SQLite** (WAL mode file DB).
- **L3 (`VectorStore` & `GraphStore: OutboundConnector`)**:
  1. **`VectorStore: OutboundConnector`** — Unified vector + metadata storage for **Turbopuffer, Qdrant, Chroma, pgvector**, etc.
  2. **`GraphStore: OutboundConnector`** — Unified knowledge-graph storage for **Neo4j** (with `source_memory_id` provenance and `valid_at`/`invalid_at` temporal edges).
  3. **`WalStore` *(optional)*** — Disk-backed write-ahead log (`redb` / SQLite) for crash-safe `IngestionBuffer` durability.

---

## 2. Crate & Module Structure (`aether-adapters`)

```text
crates/aether-adapters/
├── Cargo.toml                  # Feature flags: redis, sqlite, turbopuffer, qdrant, neo4j, chroma, pgvector
└── src/
    ├── lib.rs
    ├── config.rs               # ConnectorConfig (endpoint, SecretString, pool_size, timeouts, RetryPolicy)
    ├── registry.rs             # Config-driven builder wrapping drivers in Traced<Resilient<...>>
    ├── middleware/             # Shared cross-cutting decorators (write once, apply to all)
    │   ├── resilient.rs        # Exponential backoff + jitter on AetherError::is_retryable(), circuit breaker
    │   ├── timeout.rs          # Per-operation tokio::time::timeout
    │   ├── batcher.rs          # Auto-chunks upsert_batch using capabilities().max_batch_size
    │   └── traced.rs           # tracing spans (connector, project_id, op) + latency/error metrics
    ├── memory_test.rs          # InMemoryBackend (KvStore + VectorStore + petgraph GraphStore for cargo test)
    ├── kv/                     # redis.rs, sqlite.rs
    └── storage/                # turbopuffer.rs, qdrant.rs, neo4j.rs, chroma.rs, pgvector.rs
```

---

## 3. Shared Supertrait & Decorator Inheritance

In Rust, shared driver behavior is composed via a base supertrait (`OutboundConnector`) plus generic decorator wrappers (`Traced<Resilient<Timeout<Batcher<T>>>>`):

```rust
#[derive(Debug, Clone)]
pub struct ConnectorCapabilities {
    pub max_batch_size: usize,
    pub supports_temporal: bool,
    pub supports_filter_expr: bool,
}

#[async_trait::async_trait]
pub trait OutboundConnector: Send + Sync {
    fn name(&self) -> &'static str;
    fn capabilities(&self) -> ConnectorCapabilities;
    async fn health_check(&self) -> Result<ConnectorHealth>;
    async fn close(&self) -> Result<()>;
}
```

### How Downstream Drivers Stay Minimal
A downstream driver (e.g., `QdrantVectorStore`) only implements the raw protocol (`VectorStore`) and maps vendor errors into `AetherError`:
- **`AetherError::Transient { .. }`**: Retryable errors (HTTP 429/503, network timeouts, connection resets).
- **`AetherError::Permanent { .. }`**: Non-retryable errors (HTTP 401/403, dimension mismatch, invalid filter).

When registered via `ConnectorRegistry`, every driver is automatically wrapped in:
1. **`Batcher`** — chunks large `upsert_batch` calls according to `capabilities().max_batch_size`.
2. **`Timeout`** — bounds per-request tail latency.
3. **`Resilient`** — retries *only* `Transient` errors with exponential backoff + jitter and trips a circuit breaker on sustained outages.
4. **`Traced`** — emits `tracing` spans tagged with `connector = self.name()`, `project_id`, and `op`.

---

## 4. Contract Test Harness

Every connector—whether `InMemoryBackend`, `qdrant`, `neo4j`, or a community-contributed `pgvector` driver—must pass the same shared contract test suite in `aether-adapters/tests/`:
- **Namespace Isolation**: Writes under `ProjectId("a")` are never visible to `ProjectId("b")`.
- **Batch & Delete**: `upsert_batch` and `delete` behave idempotently.
- **Filter Translation**: `FilterExpr` (`Eq`, `In`, `Range`, `And`, `Or`) produces identical results across backends.
- **Temporal Provenance**: `retract_memory_facts` invalidates only edges owned by the forgotten `MemoryId`.
- **Transient Retry**: Simulated transient failures retry up to `max_retries`; permanent failures fail immediately.
