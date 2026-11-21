# Crates, Traits & API Reference

This reference lists the workspace crates, validated newtypes, trait contracts, orchestration methods, and error variants across Project Aether (`0.1.0`, Rust `1.89`, `edition = "2021"`).

## Workspace crates

| Crate directory | Package name | Role |
| --- | --- | --- |
| `crates/aether-core` | `muninn-aether-core` | Domain DTOs, validated IDs, scoring, filters, `AetherError`, and storage traits |
| `crates/aether-cortex` | `muninn-aether-cortex` | `Indexer` sync engine, `PersonalizedPageRank`, `graph_scores`, and `recall` functions |
| `crates/aether-adapters` | `muninn-aether-adapters` | `InMemoryBackend`, middleware wrappers, `contract` suite, and 7 backend drivers |
| `crates/aether-sdk` | `muninn-aether-sdk` | `MemoryManager` facade, `MemoryManagerBuilder`, and `telemetry` (`otel` feature) |
| `crates/aether-vault` | `muninn-aether-vault` | `VaultEngine`, `ControlPlaneServer`, `McpServer`, KV adapters, and `aether-mcp` binary |

## Validated newtypes

### Core SDK (`muninn-aether-core`)

| Type | Inner representation | Validation rule | Primary constructor |
| --- | --- | --- | --- |
| `ProjectId` | `String` | Non-empty after trim | `ProjectId::new(id)` |
| `MemoryId` | `Uuid` (`pub Uuid`) | UUID v4 or caller-provided UUID | `MemoryId::new()`, `MemoryId::from_uuid(id)` |
| `EntityId` | `String` | Non-empty after trim | `EntityId::new(id)` |
| `UserId` | `String` | Non-empty after trim | `UserId::new(id)` |
| `SessionId` | `String` | Non-empty after trim | `SessionId::new(id)` |
| `SecretString` | `String` | `Debug` and `Display` print `[REDACTED]` | `SecretString::new(secret)` |

### Obsidian Vault & Team Auth (`muninn-aether-vault`)

| Type | Format and constraints | Primary constructor |
| --- | --- | --- |
| `VaultPath` | Vault-relative POSIX `.md` path. Rejects empty, `/`, drive letters, NUL, `..`, `.obsidian`, `.git`, `.trash`, `.aether` | `VaultPath::parse(raw)` |
| `SourcePointer` | `VaultPath` plus optional single-line `#<anchor>` | `SourcePointer::parse(raw)`, `SourcePointer::new(path, anchor)` |
| `AmuId` | `amu_<32-hex>` wrapping `MemoryId` derived deterministically via SHA-256 (`aether:amu:v1:`) | `AmuId::from_pointer(&pointer)`, `AmuId::parse(raw)` |
| `CandidateId` | `cand_<6..32-hex>` (`cand_<12-hex>` when generated) | `CandidateId::from_proposal(fact, target, tier)`, `CandidateId::parse(raw)` |
| `ContentHash` | 64-character lowercase hex SHA-256 digest | `ContentHash::compute(bytes)`, `ContentHash::parse(raw)` |
| `ScopeTag` | Lowercase non-empty label with no whitespace or control characters (defaults to `global`) | `ScopeTag::new(raw)`, `ScopeTag::global()` |
| `TokenBudget` | Positive non-zero `usize` (default `1500`, `HOT_PROFILE_MAX_TOKENS` is `2000`) | `TokenBudget::new(tokens)` |
| `AgentId` | Lowercase ASCII `[a-z0-9._-]`, `1..=128` bytes | `AgentId::new(raw)` |
| `KeyId` | `key_<16..32-hex>` (`key_<16-hex>` when generated) | `KeyId::generate()`, `KeyId::parse(raw)` |
| `ApiKeySecret` | `aek_<hex>` with `>= 32` hex characters (`aek_<64-hex>` generated, non-`Serialize`, redacts `Debug` and `Display`) | `ApiKeySecret::generate()`, `ApiKeySecret::parse(raw)` |
| `ModelId` | Non-empty trimmed string without control characters | `ModelId::new(raw)` |
| `KvBlockHash` | 64-hex SHA-256 KV block identifier | `KvBlockHash::genesis()`, `paged_chained(...)`, `cache_blend(...)`, `parse(raw)` |

## Core storage traits (`muninn-aether-core`)

`Arc<T>` implements `OutboundConnector`, `RecordStore`, `VectorStore`, and `GraphStore` for any `T` implementing the corresponding trait.

### `OutboundConnector`

```rust
pub trait OutboundConnector: Send + Sync {
    fn name(&self) -> &'static str;
    fn capabilities(&self) -> ConnectorCapabilities;
    async fn health_check(&self) -> Result<ConnectorHealth>;
    async fn close(&self) -> Result<()>;
}
```

### `RecordStore: OutboundConnector`

| Method | Signature summary | Behavior |
| --- | --- | --- |
| `create` | `(ns, item, ttl) -> Result<CreateOutcome>` | Persists record and atomic `idempotency_key` (`Created` or `Duplicate(MemoryReceipt)`) |
| `get` | `(ns, id) -> Result<Option<MemoryItem>>` | Returns live non-expired, non-tombstoned record |
| `get_many` | `(ns, ids) -> Result<Vec<MemoryItem>>` | Batch lookup dropping missing, expired, or tombstoned records |
| `cas_update` | `(ns, id, expected_version, new_item) -> Result<()>` | Atomically updates live record and returns `AetherError::Conflict` on version mismatch |
| `tombstone` | `(ns, id) -> Result<Option<MemoryItem>>` | Marks deleted, bumps `version`, hides from reads while leaving pending for index retraction |
| `pending` | `(limit) -> Result<Vec<PendingRecord>>` | Cross-project unindexed records and tombstones, oldest first |
| `pending_for` | `(ns, limit) -> Result<Vec<MemoryItem>>` | Project-scoped live unindexed records for `ReadYourWrites` |
| `session_items` | `(ns, session, limit) -> Result<Vec<MemoryItem>>` | Live TTL-bound items for `SessionId` |
| `mark_indexed` | `(ns, id, version) -> Result<()>` | Advances `indexed_version` or purges tombstone if `version` still matches |

### `VectorStore: OutboundConnector`

| Method | Signature summary | Behavior |
| --- | --- | --- |
| `initialize` | `(ns, dimensions, metric) -> Result<()>` | Creates or validates namespace index with `dimensions` and `DistanceMetric` |
| `describe` | `(ns) -> Result<Option<IndexSpec>>` | Returns `{ dimensions, metric }` or `None` if uninitialized |
| `upsert_batch` | `(ns, records) -> Result<()>` | Idempotent upsert of `VectorRecord { id, vector, metadata }` |
| `delete` | `(ns, ids) -> Result<()>` | Idempotent removal by `MemoryId` |
| `search` | `(ns, query_vector, top_k, filter) -> Result<Vec<VectorMatch>>` | Returns top `k` matches optionally filtered by `FilterExpr` |

### `GraphStore: OutboundConnector`

| Method | Signature summary | Behavior |
| --- | --- | --- |
| `upsert_entities` | `(ns, entities) -> Result<()>` | Idempotent upsert of `Entity { id, name, entity_type }` |
| `upsert_relations` | `(ns, source_memory_id, relations) -> Result<()>` | Idempotent upsert keyed by `(source_memory_id, source, target, relation_type)` |
| `retract_memory_facts` | `(ns, source_memory_id, invalidated_at) -> Result<()>` | Stamps `invalid_at` on all edges belonging to `source_memory_id` |
| `neighborhood` | `(ns, seeds, hops, per_node_limit, temporal) -> Result<Subgraph>` | Bounded multi-hop traversal respecting `TemporalFilter { as_of, active_only }` |

## Vault extension traits (`muninn-aether-vault`)

| Trait | Methods | Built-in implementations |
| --- | --- | --- |
| `Embedder` | `id(&self)`, `dimensions(&self)`, `embed(&self, text)`, `device(&self)` | `HashedNgramEmbedder` (`256`-dim default), `DenseSubwordEmbedder` (`384`-dim default), `LocalGpuEmbedder` |
| `Reranker` | `id(&self)`, `top_k(&self)`, `rerank(&self, query, candidates)` | `CrossEncoderReranker` (`DEFAULT_RERANK_TOP_K = 30`), `NoopReranker` |
| `KvCacheAdapter` | `id`, `hydrate`, `warm_blocks`, `lookup_blocks`, `evict_blocks`, `stats` | `InMemoryKvCacheAdapter`, `VllmLmCacheHttpAdapter` |

## Orchestration APIs (`MemoryManager`, `Indexer`, `recall`, `VaultEngine`)

### `MemoryManager` and `MemoryManagerBuilder` (`muninn-aether-sdk`)

`MemoryManager::builder(record_store)` starts a builder with defaults `connector_tracing = true`, `session_ttl = Some(3600s)`, `timeout = Some(10s)`, `retry = Some(RetryPolicy::default())`, and `background_indexing = None`.

- **Builder methods.** `vector`, `graph`, `background_indexing`, `with_connector_tracing`, `with_session_ttl`, `with_timeout`, `with_retry`, `build`.
- **Manager methods.** `remember(MemoryInput) -> Result<MemoryReceipt>`, `recall(RecallQuery) -> Result<RecallResult>`, `update(&ProjectId, MemoryId, MemoryUpdate) -> Result<MemoryReceipt>`, `forget(&ProjectId, MemoryId) -> Result<()>`, `flush() -> Result<()>`, `shutdown() -> Result<()>`.

### `muninn-aether-cortex` (`Indexer`, `PersonalizedPageRank`, `recall`)

- **`Indexer`.** `new(record)`, `with_vector(vector)`, `with_graph(graph)`, `notify()`, `apply(&PendingRecord) -> Result<()>`, `run_pass() -> Result<PassReport>`, `spawn(&Arc<Self>, interval) -> JoinHandle<()>`.
- **`PersonalizedPageRank` and helpers.** `PersonalizedPageRank { damping: 0.85, iterations: 20 }`, `rank(&Subgraph, &[EntityId])`, `graph_scores(&Subgraph, &[(EntityId, f32)])`, `index_metadata(&MemoryItem)`.
- **`recall` module (`RRF_K = 60.0`).** `scope_filter`, `vector_search`, `unindexed`, `session`, `graph_search`, `merge`, `rrf`, `in_scopes`.

### `VaultEngine` (`muninn-aether-vault`)

| Group | Public methods |
| --- | --- |
| Lifecycle & indexing | `open(VaultConfig)`, `vault_meta()`, `wait_for_reindex()`, `sync_vault()`, `handle_fs_event(&Path)`, `status()`, `shutdown()` |
| Recall & inspection | `recall(RecallRequest)`, `recall_as(&ScopedApiKey, RecallRequest)`, `inspect_source(&str)`, `inspect_source_as(&ScopedApiKey, &str)` |
| Inbox & Dream | `stage_memory(StageMemoryRequest)`, `stage_memory_as(&ScopedApiKey, StageMemoryRequest)`, `reconcile_inbox()`, `pending_inbox()`, `pending_inbox_as(&ScopedApiKey)`, `decide_inbox_candidate(&CandidateId, bool)`, `run_dream_pass()` |
| Scoped API keys | `issue_api_key(IssueApiKeyRequest)`, `ensure_bootstrap_admin_key(AgentId, &ApiKeySecret)`, `authenticate_api_key(&ApiKeySecret)`, `list_api_keys()`, `revoke_api_key(&KeyId)` |
| Team Git & KV cache | `git_sync(GitSyncOptions)`, `warm_kv_cache(WarmKvRequest)`, `warm_kv_cache_as(&ScopedApiKey, WarmKvRequest)`, `kv_stats(Option<&ModelId>)` |

## Errors (`AetherError` and `AuthError`)

### `AetherError` (`muninn-aether-core`)

| Variant | Retryable (`is_retryable`) | Meaning |
| --- | --- | --- |
| `NotImplemented(&'static str)` | `false` | Operation not supported by the driver |
| `InvalidInput(String)` | `false` | Boundary validation, dimension mismatch, bad path, or missing store |
| `NotFound(String)` | `false` | Target memory, candidate, or source pointer missing |
| `Conflict { memory_id, expected_version, actual_version }` | `false` | Optimistic CAS version mismatch on `cas_update` |
| `Transient { connector, message }` | `true` | Timeout, HTTP 429 or 503, busy lock, or transient network error |
| `Permanent { connector, message }` | `false` | Unrecoverable backend error or wrapped `AuthError` (`connector = "vault-auth"`) |
| `Serialization(serde_json::Error)` | `false` | JSON encoding or decoding error |

### `AuthError` (`muninn-aether-vault`)

`AuthError` maps to HTTP `401 Unauthorized` or `403 Forbidden` in `ControlPlaneServer` and converts into `AetherError::Permanent { connector: "vault-auth", .. }` in Rust callers.

| Variant | HTTP status | Condition |
| --- | --- | --- |
| `MissingToken` | `401` | `require_auth` is `true` and `Authorization: Bearer ...` header is absent |
| `InvalidToken` | `401` | Malformed bearer header, invalid `aek_` format, or unknown digest |
| `RevokedToken(KeyId)` | `401` | Matched key has `revoked_at` set |
| `ExpiredToken(KeyId)` | `401` | Matched key has `expires_at <= now` |
| `InsufficientRole { required, actual }` | `403` | Caller role is below required `Reader`, `Contributor`, or `Admin` |
| `ScopeDenied { agent_id, requested }` | `403` | Requested or target `ScopeTag` not permitted by key scopes |
