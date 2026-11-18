# Aether SDK Local Development Infrastructure

Local Docker Compose stack and SQLite setup for Aether SDK development. All services bind to **unique non-default host ports** to avoid collisions with system defaults or other local services.

## Service Ports

| Service | Image | Host Port(s) | Container Port(s) | Notes |
| --- | --- | --- | --- | --- |
| PostgreSQL + pgvector | `pgvector/pgvector:pg17` | `15432` | `5432` | Auto-enables `vector` extension via `infra/postgres/init/01-pgvector.sql` |
| Qdrant (HTTP) | `qdrant/qdrant:latest` | `16333` | `6333` | REST API & Web UI (`/dashboard`) |
| Qdrant (gRPC) | `qdrant/qdrant:latest` | `16334` | `6334` | gRPC API |
| Neo4j (HTTP) | `neo4j:5-community` | `17474` | `7474` | Neo4j Browser |
| Neo4j (Bolt) | `neo4j:5-community` | `17687` | `7687` | Bolt protocol |
| Valkey | `valkey/valkey:8-alpine` | `16379` | `6379` | Redis-compatible cache/store |
| SQLite | Local file (`.aether/dev.db`) | — | — | Initialized in `WAL` journal mode |

## Quickstart

1. Copy `.env.example` to `.env` at the repository root:

   ```bash
   cp .env.example .env
   ```

2. Start all services and initialize SQLite:

   ```bash
   ./scripts/dev-infra.sh up
   ```

3. Check service status:

   ```bash
   ./scripts/dev-infra.sh status
   ```

4. Stop services (preserving data volumes):

   ```bash
   ./scripts/dev-infra.sh down
   ```

5. Reset all data (removes Docker volumes and `.aether/dev.db*`):

   ```bash
   ./scripts/dev-infra.sh reset
   ```

6. Initialize SQLite only:

   ```bash
   ./scripts/dev-infra.sh init-sqlite
   ```

## Connection Strings

Default values (matching `.env.example`):

- **PostgreSQL + pgvector**: `postgresql://aether:aether_dev@localhost:15432/aether` (`AETHER_POSTGRES_URL`)
- **Qdrant HTTP**: `http://localhost:16333` (`AETHER_QDRANT_URL`)
- **Qdrant gRPC**: `http://localhost:16334` (`AETHER_QDRANT_GRPC_URL`)
- **Neo4j Bolt**: `bolt://localhost:17687` (`AETHER_NEO4J_URI`, default auth `neo4j` / `aether_dev`)
- **Valkey**: `redis://localhost:16379` (`AETHER_VALKEY_URL`)
- **SQLite**: `.aether/dev.db` (`AETHER_SQLITE_PATH`)
