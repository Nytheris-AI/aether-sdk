---
id: 03-production-storage-backends
slug: /series/03-production-storage-backends
sidebar_position: 3
---

# Part 3: Production Storage Backends

In this tutorial, you replace `InMemoryBackend` with persistent databases from `muninn-aether-adapters`. You start the local Docker stack, enable backend Cargo features, initialize a vector `IndexSpec`, and run `MemoryManager` across SQLite, Qdrant, and Neo4j.

**Before you begin.** Complete [Part 2: Mastering the Five Composable Levels](./02-five-composable-levels.md). Install Docker, Docker Compose, and `sqlite3` (optional, for CLI WAL initialization).

## Step 1: Start the local infrastructure stack

The repository includes [`infra/docker-compose.yml`](https://github.com/muninn-hq/aether-sdk/blob/main/infra/docker-compose.yml) and [`scripts/dev-infra.sh`](https://github.com/muninn-hq/aether-sdk/blob/main/scripts/dev-infra.sh). Every service binds to a non-default host port to prevent collisions with system databases.

1. Copy `.env.example` to `.env` at the repository root:

```bash
cp .env.example .env
```

2. Start all containers and initialize `.aether/dev.db` in WAL mode:

```bash
./scripts/dev-infra.sh up
```

3. Verify that all four containers report healthy status and SQLite is present:

```bash
./scripts/dev-infra.sh status
```

| Service | Host port | Container port | Default URL or path |
| --- | --- | --- | --- |
| SQLite (local file) | `n/a` | `n/a` | `.aether/dev.db` (`WAL` mode) |
| Valkey (Redis-compatible) | `16379` | `6379` | `redis://localhost:16379` |
| PostgreSQL + `pgvector` | `15432` | `5432` | `postgresql://aether:aether_dev@localhost:15432/aether` |
| Qdrant (HTTP and gRPC) | `16333`, `16334` | `6333`, `6334` | `http://localhost:16333` |
| Neo4j (HTTP and Bolt) | `17474`, `17687` | `7474`, `7687` | `http://localhost:17474` (auth `neo4j` and `aether_dev`) |

When you finish testing, stop the containers with `./scripts/dev-infra.sh down`, or wipe volumes and `.aether/dev.db*` with `./scripts/dev-infra.sh reset`.

## Step 2: Enable backend Cargo features

All production drivers in `crates/aether-adapters/Cargo.toml` are disabled by default so you only compile the drivers you use. Enable the features you need in your `Cargo.toml`:

```toml
[dependencies]
muninn-aether-core = "0.1.0"
muninn-aether-sdk = "0.1.0"
muninn-aether-adapters = { version = "0.1.0", features = [
    "sqlite",
    "redis",
    "qdrant",
    "pgvector",
    "chroma",
    "turbopuffer",
    "neo4j",
] }
```

Each feature turns on one driver family (`sqlite` via `rusqlite`, `redis` via `redis`, `pgvector` via `sqlx` and `pgvector`, and `qdrant`, `chroma`, `turbopuffer`, `neo4j` via `reqwest`).

## Step 3: Configure `RecordStore`, `VectorStore`, and `GraphStore`

Pick one `RecordStore`, one optional `VectorStore`, and one optional `GraphStore`. All examples below return `muninn_aether_core::Result`.

### Record stores (SQLite WAL, Redis, and Valkey)

For single-node local persistence, open `SqliteRecordStore`:

```rust
use muninn_aether_adapters::kv::sqlite::{SqliteConfig, SqliteRecordStore};

let records = SqliteRecordStore::open(SqliteConfig::from_path(".aether/dev.db")).await?;
// Use SqliteConfig::in_memory() for ephemeral in-memory SQLite.
```

For shared network working memory, connect `RedisRecordStore` using Lua-atomic pending queues:

```rust
use muninn_aether_adapters::kv::redis::{RedisConfig, RedisRecordStore};
use muninn_aether_core::SecretString;

let cfg = RedisConfig::new(SecretString::new("redis://localhost:16379"), "aether-demo");
let records = RedisRecordStore::connect(cfg).await?;
```

### Vector stores (Qdrant, pgvector, Chroma, Turbopuffer)

Instantiate the vector client you run in production:

```rust
use muninn_aether_adapters::storage::qdrant::{QdrantConfig, QdrantVectorStore};

let vectors = QdrantVectorStore::new(QdrantConfig {
    base_url: "http://localhost:16333".into(),
    api_key: None,
    collection_prefix: "aether".into(),
})?;
```

To use PostgreSQL + `pgvector` instead, pass `PgVectorConfig`:

```rust
use muninn_aether_adapters::storage::pgvector::{PgVectorConfig, PgVectorStore};
use muninn_aether_core::SecretString;

let vectors = PgVectorStore::connect(PgVectorConfig {
    database_url: SecretString::new("postgresql://aether:aether_dev@localhost:15432/aether"),
    table_name: "aether_vectors".into(),
    max_connections: 8,
}).await?;
```

For Chroma v2 or Turbopuffer v1, construct `ChromaVectorStore::new(ChromaConfig::default())` or `TurbopufferVectorStore::new(TurbopufferConfig { base_url, api_key, namespace_prefix })`. See [Storage Backends, Feature Flags & Configuration](../sdk/storage-backends-and-flags.md) for every field.

### Graph store (Neo4j Query API v2)

`Neo4jGraphStore` uses Neo4j HTTP Query API v2 (`POST /db/{database}/query/v2`) on port `17474`:

```rust
use muninn_aether_adapters::storage::neo4j::{Neo4jConfig, Neo4jGraphStore};
use muninn_aether_core::SecretString;

let graph = Neo4jGraphStore::new(Neo4jConfig {
    base_url: "http://localhost:17474".into(),
    database: "neo4j".into(),
    username: "neo4j".into(),
    password: SecretString::new("aether_dev"),
})?;
```

## Step 4: Initialize `IndexSpec` and verify `describe()`

Before calling `MemoryManager::remember`, initialize the vector index for each `ProjectId` and verify that `describe()` returns the persisted `IndexSpec`:

```rust
use muninn_aether_core::{DistanceMetric, ProjectId, VectorStore};

let ns = ProjectId::new("prod-app")?;
vectors.initialize(&ns, 3, DistanceMetric::Cosine).await?;

let spec = vectors.describe(&ns).await?.expect("initialized spec");
assert_eq!(spec.dimensions, 3);
assert_eq!(spec.metric, DistanceMetric::Cosine);
spec.check(&[1.0, 0.0, 0.0])?;
```

`QdrantVectorStore`, `PgVectorStore`, and `ChromaVectorStore` persist `dimensions` and `metric` in backend metadata or a `<table_name>_specs` table so restarted processes recover `IndexSpec` automatically. `TurbopufferVectorStore` caches `IndexSpec` in process memory after `initialize`, so restarted processes must call `initialize` once per namespace before `remember` or `recall`.

## Step 5: Assemble `MemoryManager` and run the live test suite

Wire the three connectors into `MemoryManager::builder`:

```rust
use std::sync::Arc;
use muninn_aether_sdk::MemoryManager;

let manager = MemoryManager::builder(Arc::new(records))
    .vector(Arc::new(vectors))
    .graph(Arc::new(graph))
    .build();
```

You can verify the full adapter contract suite offline and against your running Docker stack:

```bash
# Offline unit and mocked HTTP contract tests across all features
cargo test -p muninn-aether-adapters --all-features

# Live Docker end-to-end tests against Valkey, Qdrant, pgvector, and Neo4j
QDRANT_URL=http://127.0.0.1:16333 \
NEO4J_URL=http://127.0.0.1:17474 \
NEO4J_PASSWORD=aether_dev \
PGVECTOR_URL=postgresql://aether:aether_dev@127.0.0.1:15432/aether \
REDIS_URL=redis://127.0.0.1:16379 \
cargo test -p muninn-aether-sdk --test live_docker --features live-docker -- --ignored
```

**Next in series.** Continue to [Part 4: Obsidian Vault Compiler & MCP Server](./04-obsidian-vault-and-mcp.md).
