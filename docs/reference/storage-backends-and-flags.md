# Storage Backends, Feature Flags & Configuration

This reference covers Cargo feature flags, connector configuration structs, `ConnectorCapabilities`, middleware defaults, and local Docker infrastructure settings.

## Cargo feature matrix

### `muninn-aether-adapters` (`crates/aether-adapters/Cargo.toml`)

Default features are empty (`default = []`). `InMemoryBackend` and middleware wrappers (`Batcher`, `Timeout`, `Resilient`, `Traced`) compile with no feature flags.

| Feature | Enables | Dependencies | Contract integration test |
| --- | --- | --- | --- |
| `sqlite` | `kv::sqlite::{SqliteConfig, SqliteRecordStore}` | `rusqlite` (`bundled`) | `driver_sqlite` |
| `redis` | `kv::redis::{RedisConfig, RedisKeyLayout, RedisRecordStore}` | `redis` (`tokio-comp`, `connection-manager`) | `driver_redis` |
| `qdrant` | `storage::qdrant::{QdrantConfig, QdrantVectorStore}` | `reqwest` (`json`, `rustls`) | `driver_qdrant` |
| `pgvector` | `storage::pgvector::{PgVectorConfig, PgVectorStore}` | `sqlx` (`postgres`, `runtime-tokio`, `tls-rustls`), `pgvector` | `driver_pgvector` |
| `chroma` | `storage::chroma::{ChromaConfig, ChromaVectorStore}` | `reqwest` (`json`, `rustls`) | `driver_chroma` |
| `turbopuffer` | `storage::turbopuffer::{TurbopufferConfig, TurbopufferVectorStore}` | `reqwest` (`json`, `rustls`) | `driver_turbopuffer` |
| `neo4j` | `storage::neo4j::{Neo4jConfig, Neo4jGraphStore}` | `reqwest` (`json`, `rustls`) | `driver_neo4j` |
| `contract-suite` | `contract::{record_store, record_store_concurrent_idempotency, vector_store, graph_store}` | None | Used by all `driver_*` tests |

### `muninn-aether-sdk` (`crates/aether-sdk/Cargo.toml`)

| Feature | Enables | Dependencies |
| --- | --- | --- |
| `otel` | `telemetry::{init_otel, LogFormat, OtelConfig, TelemetryGuard}` and `otel_demo` example | `opentelemetry`, `opentelemetry_sdk`, `opentelemetry-otlp`, `opentelemetry-appender-tracing`, `tracing-opentelemetry`, `tracing-subscriber` |
| `live-docker` | Enables `sqlite`, `redis`, `qdrant`, `chroma`, `pgvector`, `neo4j` for `tests/live_docker.rs` | `muninn-aether-adapters` driver features |

## Backend configurations and constructors

| Store | Trait | Config struct and fields | Constructors and helper methods |
| --- | --- | --- | --- |
| `InMemoryBackend` | `RecordStore`, `VectorStore`, `GraphStore` | None | `InMemoryBackend::new()`, `fail_next(FaultPoint, usize)`, `has_vector(&ProjectId, MemoryId)` |
| `SqliteRecordStore` | `RecordStore` | `SqliteConfig { path: String }` | `SqliteConfig::in_memory()`, `SqliteConfig::from_path(path)`, `SqliteRecordStore::open(cfg)` |
| `RedisRecordStore` | `RecordStore` | `RedisConfig { url: SecretString, key_prefix: String }` | `RedisConfig::new(url, prefix)`, `validate()`, `key_layout()`, `RedisRecordStore::connect(cfg)` |
| `QdrantVectorStore` | `VectorStore` | `QdrantConfig { base_url: String, api_key: Option<SecretString>, collection_prefix: String }` | `QdrantConfig::default()` (`http://localhost:6333`, `"aether"`), `QdrantVectorStore::new(cfg)` |
| `PgVectorStore` | `VectorStore` | `PgVectorConfig { database_url: SecretString, table_name: String, max_connections: u32 }` | `PgVectorConfig::validate()`, `connect(cfg)`, `connect_lazy(cfg)`, `from_pool(pool, table)` |
| `ChromaVectorStore` | `VectorStore` | `ChromaConfig { base_url: String, tenant: String, database: String, auth_token: Option<SecretString>, collection_prefix: String }` | `ChromaConfig::default()` (`http://localhost:8000`, `"default_tenant"`, `"default_database"`, `"aether"`), `ChromaVectorStore::new(cfg)` |
| `TurbopufferVectorStore` | `VectorStore` | `TurbopufferConfig { base_url: String, api_key: SecretString, namespace_prefix: String }` | `TurbopufferVectorStore::new(cfg)` |
| `Neo4jGraphStore` | `GraphStore` | `Neo4jConfig { base_url: String, database: String, username: String, password: SecretString }` | `Neo4jGraphStore::new(cfg)`, `Neo4jGraphStore::with_client(cfg, reqwest_client)` |

## `ConnectorCapabilities` and `describe()` behavior

| Connector (`name()`) | `max_batch_size` | `supports_temporal` | `supports_filter_expr` | Cross-restart `VectorStore::describe` |
| --- | --- | --- | --- | --- |
| `in_memory` | `1024` | `true` | `true` | In-memory only |
| `sqlite` | `512` | `false` | `false` | `n/a` (`RecordStore`) |
| `redis` | `256` | `false` | `false` | `n/a` (`RecordStore`) |
| `qdrant` | `256` | `false` | `true` | Persisted on collection config |
| `pgvector` | `1000` | `false` | `true` | Persisted in `<table_name>_specs` |
| `chroma` | `1000` | `false` | `true` | Persisted in collection metadata (`_aether_dimensions`, `hnsw:space`) |
| `turbopuffer` | `1000` | `false` | `true` | Process-cached after `initialize` |
| `neo4j` | `1000` | `true` | `false` | `n/a` (`GraphStore`) |

## Middleware and OpenTelemetry (`otel`)

`MemoryManager::builder` wraps stores in `Batcher` (vector only), `Timeout`, `Resilient`, and `Traced`.

| Wrapper | Type | Defaults in `MemoryManager::builder` |
| --- | --- | --- |
| `Batcher<T>` | Vector batch chunker | Splits `upsert_batch` by `capabilities().max_batch_size.max(1)` |
| `Timeout<T>` | Tail-latency guard | `10s`, mapping elapsed timeouts to `AetherError::Transient` |
| `Resilient<T>` | Retry decorator | `RetryPolicy { max_retries: 3, base_delay: 50ms }` with `jittered` full jitter |
| `Traced<T>` | OpenTelemetry span emitter | Enabled (`with_connector_tracing(true)`), emitting `connector_op` spans |

`OtelConfig::from_env(service_name)` reads two standard environment variables:

- `OTEL_EXPORTER_OTLP_ENDPOINT` (OTLP gRPC exporter endpoint, defaulting to `http://localhost:4317` in Tonic).
- `RUST_LOG` (subscriber filter, falling back to `info`).
- `LogFormat` options are `Pretty` (default), `Json`, and `None`. `otlp_logs` defaults to `true`.

## Local Docker Compose stack (`infra/` and `scripts/dev-infra.sh`)

`./scripts/dev-infra.sh` accepts `up`, `down`, `status`, `reset`, and `init-sqlite`. It loads `.env` if present and invokes `infra/docker-compose.yml`.

| Compose service | Image | Container name | Host port(s) | Container port(s) | `.env.example` variables |
| --- | --- | --- | --- | --- | --- |
| `postgres` | `pgvector/pgvector:pg17` | `aether-postgres` | `15432` | `5432` | `AETHER_POSTGRES_USER` (`aether`), `AETHER_POSTGRES_PASSWORD` (`aether_dev`), `AETHER_POSTGRES_DB` (`aether`), `AETHER_POSTGRES_PORT` (`15432`), `AETHER_POSTGRES_URL` |
| `qdrant` | `qdrant/qdrant:latest` | `aether-qdrant` | `16333`, `16334` | `6333`, `6334` | `AETHER_QDRANT_HTTP_PORT` (`16333`), `AETHER_QDRANT_GRPC_PORT` (`16334`), `AETHER_QDRANT_URL`, `AETHER_QDRANT_GRPC_URL` |
| `neo4j` | `neo4j:5-community` | `aether-neo4j` | `17474`, `17687` | `7474`, `7687` | `AETHER_NEO4J_USER` (`neo4j`), `AETHER_NEO4J_PASSWORD` (`aether_dev`), `AETHER_NEO4J_HTTP_PORT` (`17474`), `AETHER_NEO4J_BOLT_PORT` (`17687`), `AETHER_NEO4J_URI` |
| `valkey` | `valkey/valkey:8-alpine` | `aether-valkey` | `16379` | `6379` | `AETHER_VALKEY_PORT` (`16379`), `AETHER_VALKEY_URL` |
| SQLite (host file) | Local `sqlite3` | `n/a` | `n/a` | `n/a` | `AETHER_SQLITE_PATH` (`.aether/dev.db`) |

PostgreSQL startup executes `infra/postgres/init/01-pgvector.sql` (`CREATE EXTENSION IF NOT EXISTS vector;`).
