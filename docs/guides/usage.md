# Usage Guide

Aether SDK can be entered at any of five levels depending on how much of the pipeline you want to own. Every example below is runnable verbatim with `cargo run -p nytheris-aether-sdk --example <name>`.

---

## Level 1: Direct Connector (`level_1_connector.rs`)

Use a single `RecordStore`, `VectorStore`, or `GraphStore` directly without middleware, indexers, or `MemoryManager`:

```rust
use nytheris_aether_adapters::InMemoryBackend;
use nytheris_aether_core::{DistanceMetric, MemoryId, ProjectId, VectorRecord, VectorStore};
use serde_json::json;

#[tokio::main]
async fn main() -> nytheris_aether_core::Result<()> {
    let store = InMemoryBackend::new();
    let ns = ProjectId::new("level1-demo")?;
    store.initialize(&ns, 3, DistanceMetric::Cosine).await?;

    let id = MemoryId::new();
    store
        .upsert_batch(
            &ns,
            &[VectorRecord {
                id,
                vector: vec![1.0, 0.0, 0.0],
                metadata: json!({"topic": "connectors"}),
            }],
        )
        .await?;

    let hits = store.search(&ns, &[1.0, 0.0, 0.0], 5, None).await?;
    println!("level 1 hits: {} (top id={})", hits.len(), hits[0].id);
    Ok(())
}
```

---

## Level 2: Middleware Decorators (`level_2_middleware.rs`)

Wrap any connector in `Batcher`, `Timeout`, `Resilient`, and `Traced`:

```rust
use std::sync::Arc;
use std::time::Duration;
use nytheris_aether_adapters::{Batcher, FaultPoint, InMemoryBackend, Resilient, RetryPolicy, Timeout};
use nytheris_aether_core::{DistanceMetric, MemoryId, ProjectId, VectorRecord, VectorStore};
use serde_json::json;

#[tokio::main]
async fn main() -> nytheris_aether_core::Result<()> {
    let raw = Arc::new(InMemoryBackend::new());
    let ns = ProjectId::new("level2-demo")?;
    raw.initialize(&ns, 2, DistanceMetric::Cosine).await?;

    let store = Resilient::new(
        Timeout::new(Batcher::new(raw.clone()), Duration::from_secs(2)),
        RetryPolicy {
            max_retries: 3,
            base_delay: Duration::from_millis(5),
        },
    );

    raw.fail_next(FaultPoint::VectorUpsert, 1);
    store
        .upsert_batch(
            &ns,
            &[VectorRecord {
                id: MemoryId::new(),
                vector: vec![1.0, 0.0],
                metadata: json!({"retry": true}),
            }],
        )
        .await?;

    let hits = store.search(&ns, &[1.0, 0.0], 5, None).await?;
    println!("level 2 hits after retry: {}", hits.len());
    Ok(())
}
```

---

## Level 3: Composable Hybrid Recall (`level_3_hybrid_recall.rs`)

Call `nytheris_aether_cortex::recall::{vector_search, graph_search, rrf, RRF_K}` and `PersonalizedPageRank` directly to fuse dense vector search with graph traversal:

```rust
let vec_cands =
    recall::vector_search(&backend, &backend, &ns, &[1.0, 0.0], 5, None).await?;
let (graph_cands, subgraph) = recall::graph_search(
    &backend,
    &backend,
    &ns,
    &[alice.id],
    2,
    10,
    &TemporalFilter::default(),
    &PersonalizedPageRank::default(),
)
.await?;

let fused = recall::rrf(&[vec_cands, graph_cands], RRF_K);
```

---

## Level 4: Durable Indexer Sync Engine (`level_4_sync_engine.rs`)

Write authoritative records to a `RecordStore`, then drive derived vector and graph updates explicitly via `Indexer::run_pass()` (or opt into an interval loop with `Indexer::spawn`):

```rust
let indexer = Indexer::new(backend.clone())
    .with_vector(backend.clone())
    .with_graph(backend.clone());
let report = indexer.run_pass().await?;
```

---

## Level 5: `MemoryManager` Facade (`level_5_facade.rs`)

Assemble an authoritative `RecordStore` with optional vector and graph indexes using `MemoryManager::builder`:

```rust
use std::sync::Arc;
use nytheris_aether_adapters::InMemoryBackend;
use nytheris_aether_core::{
    DistanceMetric, Entity, EntityId, MemoryInput, MemoryUpdate, ProjectId, RecallMode,
    RecallQuery, Relation, VectorStore, WriteDurability,
};
use nytheris_aether_sdk::MemoryManager;
use chrono::Utc;

#[tokio::main]
async fn main() -> nytheris_aether_core::Result<()> {
    let backend = Arc::new(InMemoryBackend::new());
    let ns = ProjectId::new("level5-demo")?;
    backend.initialize(&ns, 2, DistanceMetric::Cosine).await?;

    let manager = MemoryManager::builder(backend.clone())
        .vector(backend.clone())
        .graph(backend)
        .build();

    let alice = Entity {
        id: EntityId::new("person:alice")?,
        name: "Alice".into(),
        entity_type: "Person".into(),
    };
    let mut input = MemoryInput::new(ns.clone(), "Alice prefers Qdrant");
    input.embedding = Some(vec![1.0, 0.0]);
    input.durability = WriteDurability::Buffered;
    input.entities = vec![alice.clone()];
    input.relations = vec![Relation {
        source: alice.id.clone(),
        target: EntityId::new("db:qdrant")?,
        relation_type: "PREFERS".into(),
        valid_at: Utc::now(),
        invalid_at: None,
    }];
    let receipt = manager.remember(input).await?;
    manager.flush().await?;

    let mut q = RecallQuery::new(ns.clone(), "What database does Alice prefer?");
    q.query_vector = Some(vec![1.0, 0.0]);
    q.seed_entities = vec![alice.id];
    q.mode = RecallMode::Hybrid;
    let res = manager.recall(q).await?;
    println!("level 5 recalled hits: {}, edges: {}", res.hits.len(), res.subgraph.edges.len());

    let updated = manager
        .update(
            &ns,
            receipt.memory_id,
            MemoryUpdate {
                content: Some("Alice now prefers pgvector".into()),
                embedding: Some(vec![0.0, 1.0]),
                metadata: None,
                entities: None,
                relations: None,
                expected_version: Some(receipt.version),
            },
        )
        .await?;
    println!("level 5 updated version: {}", updated.version);

    manager.forget(&ns, receipt.memory_id).await?;
    manager.shutdown().await?;
    Ok(())
}
```

---

## Obsidian Vault & MCP Server (`nytheris-aether-vault`)

Use `nytheris-aether-vault` when you want an Obsidian-native local memory daemon backed by a single SQLite WAL database and a 3-tool `stdio` MCP server.

### Run the `aether-mcp` Stdio & Control-Plane Server

```bash
# 1. Install the Obsidian companion plugin and print an mcp.json snippet
cargo run -p nytheris-aether-vault --bin aether-mcp -- \
  --init --vault /path/to/obsidian-vault --http-addr 127.0.0.1:14771

# 2. Loopback Obsidian HTTP daemon (stays alive until Ctrl-C / SIGTERM without open stdin)
cargo run -p nytheris-aether-vault --bin aether-mcp -- \
  --vault /path/to/obsidian-vault \
  --db /path/to/aether.sqlite \
  --watch-ms 2000 \
  --http-addr 127.0.0.1:14771 \
  --http-only

# 3. Team HTTP gateway + KV-cache warming + scoped stdio MCP mode
cargo run -p nytheris-aether-vault --bin aether-mcp -- \
  --vault /path/to/team-vault \
  --http-addr 0.0.0.0:4771 \
  --require-auth \
  --bootstrap-admin-key "$AETHER_BOOTSTRAP_ADMIN_KEY" \
  --api-key "$AETHER_API_KEY" \
  --kv-endpoint http://127.0.0.1:8000 \
  --kv-model llama-3.1-8b
```

When `--vault` is omitted, `aether-mcp` reads `AETHER_VAULT_PATH` or falls back to `.`. It serves newline-delimited JSON-RPC 2.0 (`protocolVersion` `"2024-11-05"`) over `stdin` and `stdout` with three tools. Passing `--api-key` (or `AETHER_API_KEY`) binds the stdio session to a validated `ScopedApiKey`.

- `aether_recall` accepts `query` (required), `scope` (optional), `max_tokens` (optional, default `1500`), `include_hot_profile` (optional, default `true`), `structured` (optional, default `false` to emit JSON separating `static_prefix`, `dynamic_context`, and `prefix_hash`; `structuredContent` is always included), and `kv_model` (optional target model for `kv_hint` block reuse metadata).
- `aether_stage_memory` accepts `proposed_fact` (required), `suggested_tier` (optional `"hot"`, `"warm"`, or `"cold"`, default `"warm"`), `scope` (optional, defaulting to `agent.default_write_scope()` on scoped sessions), `target_note` (optional), and `supersedes_amu_id` (optional). It appends a pending candidate to `Aether/Inbox.md`.
- `aether_inspect_source` accepts `source_pointer` (required, for example `"Arch/DB.md#Schema"` or `"Aether/Compiled/Approved.md#cand_9f82a1"`). It returns the verbatim Markdown section from disk while enforcing `.aetherignore`, `aether_tier: private`, and scoped agent read permissions.

### Team HTTP Gateway & Obsidian Companion Plugin

Without `--require-auth`, `ControlPlaneServer::bind` rejects non-loopback addresses (`127.0.0.1` and `::1` only). Passing `--require-auth` (or calling `ControlPlaneServer::bind_with_auth`) enables non-loopback binds, requires `Authorization: Bearer aek_<hex>` on every data route (`GET /v1/openapi.json` stays publicly readable for Custom GPT Actions and Open WebUI schema import), and exposes the full endpoint surface.

- Local management and Inbox endpoints expose `GET /v1/openapi.json` (public OpenAPI `3.1.0`), `GET /v1/status`, `GET /v1/inbox`, `POST /v1/inbox/decide` (`Admin`), `POST /v1/inbox/reconcile` (`Admin`), `POST /v1/dream` (`Admin`), and `POST /v1/sync` (`Admin`).
- Team memory, MCP, Git, and KV endpoints expose `POST /v1/recall` (`Reader+`), `POST /v1/stage` (`Contributor+`), `POST /v1/inspect` (`Reader+`), `POST /v1/mcp` (`Reader+`), `POST /v1/git/sync` (`Admin`), `POST /v1/kv/warm` (`Contributor+`), and `GET /v1/kv/stats` (`Reader+`).
- Key management endpoints (`Admin`) expose `GET /v1/auth/keys`, `POST /v1/auth/keys`, and `POST /v1/auth/keys/revoke`.

The TypeScript plugin in [`obsidian-aether-plugin`](../../obsidian-aether-plugin/manifest.json) supports `AetherClientOptions` (`apiKey`, `allowInsecureRemoteHttp`, `defaultKvModel`), enforces `https://` for remote team gateways unless `allowInsecureRemoteHttp` is enabled, and registers `aether-git-sync` and `aether-warm-kv-cache` commands.

### Embed `VaultEngine` and Team Primitives in Rust

```rust
use std::sync::Arc;
use nytheris_aether_vault::{
    AgentId, AgentRole, DenseSubwordEmbedder, GitSyncOptions, IssueApiKeyRequest,
    McpServer, ModelId, RecallRequest, ScopeTag, StageMemoryRequest, TokenBudget,
    VaultConfig, VaultEngine, VaultTier, WarmKvRequest,
};

#[tokio::main]
async fn main() -> nytheris_aether_core::Result<()> {
    let config = VaultConfig::new("/path/to/obsidian-vault")
        .with_embedder(Arc::new(DenseSubwordEmbedder::default()))
        .with_default_kv_model(ModelId::new("llama-3.1-8b")?);
    let engine = Arc::new(VaultEngine::open(config).await?);
    engine.sync_vault().await?;

    let issued = engine
        .issue_api_key(IssueApiKeyRequest::new(
            AgentId::new("coding-agent")?,
            AgentRole::Contributor,
            vec![ScopeTag::new("engineering")?],
        )?)
        .await?;

    let warm = engine
        .warm_kv_cache_as(&issued.key, WarmKvRequest::new(ModelId::new("llama-3.1-8b")?))
        .await?;
    println!("warmed {} KV blocks ({:.1}% TTFT reduction)", warm.blocks_warmed, warm.ttft_reduction_pct);

    let mut req = RecallRequest::new("SQLite WAL transactions");
    req.max_tokens = TokenBudget::new(1500)?;
    let recall = engine.recall_as(&issued.key, req).await?;
    println!(
        "tokens={}, static_tokens={}, prefix_hash={}, kv_hit_tokens={:?}",
        recall.tokens_used,
        recall.static_prefix_tokens,
        recall.prefix_hash,
        recall.kv_hint.as_ref().map(|h| h.cache_hit_tokens),
    );

    let mut staged_req = StageMemoryRequest::new(
        "Use TransactionBehavior::Immediate for writes.",
        VaultTier::Warm,
    )
    .with_target_note("Arch/DB.md");
    staged_req.scope = issued.key.default_write_scope();
    let candidate_id = engine.stage_memory_as(&issued.key, staged_req).await?;
    println!("staged {candidate_id} into Aether/Inbox.md");

    let git_report = engine.git_sync(GitSyncOptions::new()).await?;
    println!("git sync cleanly={}, resolved={}", git_report.merged_cleanly, git_report.aether_conflicts_resolved);

    let _server = McpServer::with_api_key(Arc::clone(&engine), &issued.secret).await?;
    engine.shutdown().await?;
    Ok(())
}
```

### Note Frontmatter and Exclusions

Notes support optional YAML frontmatter keys `aether_tier` (`hot`, `warm`, `cold`, `private`), `aether_scope`, `aether_kind` (`profile`, `rule`, `decision`, `spec`, `fleeting`), `aether_status` (`active`, `superseded`, `archived`), `valid_from`, `superseded_by`, and `tags`. Built-in exclusions skip `.obsidian`, `.git`, `.trash`, `.aether`, `Aether/Inbox.md`, `Aether/Compiled`, `.canvas`, `.excalidraw`, and `.excalidraw.md`; add custom rules in `.aetherignore` at the vault root.
