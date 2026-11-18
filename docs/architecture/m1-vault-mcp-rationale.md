# Project Aether Milestone 1 vault and MCP architecture

## Problem

Milestone 1 builds a local Obsidian vault daemon and a three-tool `stdio` MCP server inside a new workspace crate at `crates/aether-vault`, named `nytheris-aether-vault`. Existing crates in Aether SDK separate storage contracts from graph and fusion ranking. `nytheris-aether-adapters` provides `SqliteRecordStore` for opaque JSON payloads without FTS5 full-text search, wikilink edge queries, or a self-write hash ledger.

Offline `Cargo.lock` constraints also shape the build boundary. The lockfile includes `rusqlite` `0.41` with bundled SQLite and FTS5, `sha2` `0.11`, `hex` `0.4`, `walkdir` `2.5`, `tokio`, `serde_json`, `chrono`, `petgraph`, `thiserror`, `tracing`, and `uuid` without the `v5` feature. Neither `notify`, `rmcp`, nor `serde_yaml` is present in `Cargo.lock`. The crate must parse Obsidian notes, exclude `private` notes at the boundary, fuse FTS5, local vectors, and `PersonalizedPageRank` scores into deterministic `HOT` prompt prefixes, reconcile `Aether/Inbox.md` approvals without watcher loops, and expose the three MCP tools without leaking internal database or JSON-RPC types.

## Usage

Consumers depend on `nytheris-aether-vault` and interact with `VaultEngine`, `McpServer`, and validated domain types.

### Quickstart daemon and MCP server

```rust
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use nytheris_aether_core::Result;
use nytheris_aether_vault::{McpServer, VaultConfig, VaultEngine};

#[tokio::main]
async fn main() -> Result<()> {
	let vault_root = PathBuf::from(std::env::args().nth(1).expect("vault path"));
	let config = VaultConfig::new(&vault_root)
		.with_watch_interval(Some(Duration::from_secs(2)));

	let engine = Arc::new(VaultEngine::open(config).await?);
	let server = McpServer::new(Arc::clone(&engine));
	server.serve_stdio().await?;

	engine.shutdown().await
}
```

### Call site 1. Hybrid recall with deterministic prefix caching

```rust
use nytheris_aether_vault::{RecallRequest, ScopeTag, TokenBudget, VaultConfig, VaultEngine};

async fn build_context(vault_root: &std::path::Path) -> nytheris_aether_core::Result<String> {
	let engine = VaultEngine::open(VaultConfig::in_memory(vault_root)).await?;
	engine.sync_vault().await?;

	let mut req = RecallRequest::new("SQLite write locking and WAL transactions");
	req.scope = Some(ScopeTag::new("engineering")?);
	req.max_tokens = TokenBudget::new(1500)?;
	req.include_hot_profile = true;

	let response = engine.recall(req).await?;
	assert!(response.tokens_used <= 1500);
	let _stable_hash = response.prefix_hash;
	Ok(response.formatted_context)
}
```

### Call site 2. Non-destructive staging into the Inbox

```rust
use nytheris_aether_vault::{StageMemoryRequest, VaultConfig, VaultEngine, VaultTier};

async fn propose_fact(engine: &VaultEngine) -> nytheris_aether_core::Result<()> {
	let candidate_id = engine
		.stage_memory(
			StageMemoryRequest::new(
				"Use TransactionBehavior::Immediate for all SQLite writes.",
				VaultTier::Warm,
			)
			.with_scope("engineering")?
			.with_target_note("Architecture/Storage.md"),
		)
		.await?;

	assert!(candidate_id.as_str().starts_with("cand_"));
	Ok(())
}
```

### Call site 3. Human approval and verbatim source inspection

```rust
use nytheris_aether_core::AetherError;
use nytheris_aether_vault::{SourcePointer, VaultEngine};

async fn approve_and_verify(engine: &VaultEngine) -> nytheris_aether_core::Result<String> {
	let report = engine.sync_vault().await?;
	assert!(report.inbox_approved <= report.files_scanned + report.inbox_approved);

	let ptr = SourcePointer::parse("Architecture/Storage.md#WAL-Mode")?;
	let verbatim = engine.inspect_source(ptr.as_str()).await?;

	assert!(matches!(
		SourcePointer::parse("../../etc/passwd"),
		Err(AetherError::InvalidInput(_))
	));
	Ok(verbatim)
}
```

## Shape

The design places one deep owner, `VaultEngine`, over an internal SQLite WAL store, pure Markdown parser, deterministic prefix formatter, and atomic Inbox reconciler. Public callers see `VaultEngine`, `McpServer`, and validated request and response types. Storage schemas, `rusqlite` connections, `RawFrontmatterTier`, and JSON-RPC wire structs stay private to the crate.

### Module map

```text
crates/aether-vault/
├── Cargo.toml
└── src/
    ├── lib.rs             # Public re-exports only
    ├── types.rs           # Branded primitives, AMU model, request and response types
    ├── parser.rs          # IgnoreRules, YAML frontmatter, ATX splitter, Embedder trait
    ├── inbox.rs           # Atomic writes and PRD Section 7.3 Inbox parser and renderer
    ├── prefix.rs          # Deterministic HOT prefix builder and dynamic token packer
    ├── store.rs           # Internal SQLite WAL, FTS5, graph edges, and self-write ledger
    ├── engine.rs          # VaultEngine orchestrator and background polling loop
    ├── mcp.rs             # Thin JSON-RPC 2.0 MCP handler and stdio stream loop
    └── bin/
        └── aether-mcp.rs  # CLI daemon entrypoint
```

### Core data structures and boundary invariants

- **`SourcePointer` and `VaultPath` (`types.rs`).** Both normalize backslashes to forward slashes, strip leading `./` segments, and reject empty paths, leading slashes, NUL bytes, `..` traversal segments, and hidden system directories such as `.obsidian/`, `.git/`, and `.trash/` (`per principle-boundary-discipline`).
- **`AmuId` (`types.rs`).** Derives a 128-bit UUID from the SHA-256 digest of the canonical `SourcePointer` stamped with RFC 4122 version and variant bits, mapping one-to-one to `nytheris_aether_core::MemoryId` without requiring the `uuid` `v5` feature (`per principle-type-system-discipline`).
- **`CandidateId`, `ContentHash`, `ScopeTag`, and `TokenBudget` (`types.rs`).** Each validates its format (`cand_` with 12 lowercase hex characters, 64 lowercase hex characters, non-empty lowercase scope label, and positive token ceiling) at construction time.
- **`VaultTier` and `RawFrontmatterTier` (`types.rs`, `parser.rs`).** Public `VaultTier` contains only `Hot`, `Warm`, and `Cold`. `Private` exists only on the crate-private `RawFrontmatterTier` inside `parser.rs`, so private notes cannot enter `StageMemoryRequest`, SQLite storage, or recall results (`per principle-type-system-discipline`).
- **`Amu` and `AmuStatus` (`types.rs`).** Captures `amu_id`, `tier`, `kind`, `status`, `distilled_fact`, `source_pointer`, `graph_edges`, `valid_from`, `superseded_by`, `scopes`, `authority_weight`, and `content_hash`.

### Ingestion, concurrency, and recall flow

1. **Parsing and exclusion (`parser.rs`).** `IgnoreRules` loads `.aetherignore` once and enforces one shared list of built-in exclusions across directory scans and `inspect_source` checks. `parse_note` extracts YAML frontmatter, returns `ParsedNote::Private` immediately when `aether_tier` is `private`, splits body lines at ATX headings outside fenced code blocks, emits `#_preamble` when text precedes the first heading, extracts `[[wikilinks]]` and `#tags`, and scores `authority_weight` with evergreen notes above `Daily` or `Journal` notes and unchecked task lists.
2. **Storage and loop prevention (`store.rs`, `engine.rs`).** `VaultStore` opens one SQLite WAL connection wrapped in `Arc<Mutex<Option<Connection>>>` and runs queries through `tokio::task::spawn_blocking` with `TransactionBehavior::Immediate` for writes (`per principle-separate-before-serializing-shared-state`). `sync_vault`, `handle_fs_event`, `stage_memory`, and `reconcile_inbox` serialize vault file mutations under `write_gate` while leaving `recall` and `inspect_source` lock-free under WAL snapshots. Every atomic write computes its `ContentHash`, inserts `(rel_path, hash)` into `self_write_ledger` before `rename`, and skips matching self-originated hashes on later scans while pruning stale ledger entries (`per principle-make-operations-idempotent`).
3. **Hybrid retrieval and prefix packing (`engine.rs`, `prefix.rs`).** `VaultEngine::recall` supports empty queries when `include_hot_profile` is `true` by returning only the static `HOT` prefix. For non-empty queries, `VaultEngine` queries SQLite FTS5 BM25, scores active rows with local 256-dim vectors from `HashedNgramEmbedder` via `DistanceMetric::Cosine`, builds a deduplicated `Subgraph` for `PersonalizedPageRank` and `graph_scores`, and fuses all three ranked lists with `nytheris_aether_cortex::recall::rrf` scaled by `authority_weight`. `prefix.rs` sorts `HOT` items strictly by `amu_id` ascending inside a 2,000-token ceiling with zero volatile timestamps, computes `prefix_hash`, and appends dynamic `WARM` and `COLD` items within `max_tokens`.
4. **Inbox staging and MCP server (`inbox.rs`, `mcp.rs`).** `stage_memory` formats proposals in the PRD Section 7.3 Markdown checkbox shape and replaces `Aether/Inbox.md` via temp-file write, `sync_all`, and `rename`. `reconcile_inbox` compiles checked `[x]` items into active SQLite AMUs, marks any `supersedes_amu_id` target as `Superseded`, drops `[-]` items, and rewrites the remaining pending items atomically. `McpServer` dispatches `initialize`, `ping`, and `tools/list` plus `tools/call` for `aether_recall`, `aether_stage_memory`, and `aether_inspect_source`.

### Interface depth verdict

Callers learn two primary types (`VaultEngine` and `McpServer`) and small value objects. Behind that boundary sit file walking, `.aetherignore` matching, YAML and Markdown parsing, SQLite WAL storage with FTS5, feature-hash vector scoring, PageRank graph ranking, RRF fusion, canonical prefix caching, and atomic Inbox reconciliation (`per principle-minimize-reader-load`). Every MCP tool call completes within two internal hops.

## Synthesis decision

Candidate 1 (`VaultEngine` single-owner design) won the Phase C evaluation with a score of 23 out of 25 against Candidate 2's score of 16 out of 25, and Candidate 1 becomes the base architecture. Candidate 2 split state across `records.db` via `SqliteRecordStore` and `vault.db`, which hit two structural conflicts: `MemoryScope::index_key` only represents `"project"` and `"user:<id>"` rather than vault topic scopes, and `SqliteRecordStore::mark_indexed` deletes tombstones while `vault.db` keeps superseded metadata. Candidate 1 avoids split ownership by committing AMUs, FTS5 rows, graph edges, self-write hashes, and Inbox state inside one SQLite WAL transaction.

Three parts from Candidate 2 were grafted into the Candidate 1 base:

1. **Internal `Embedder` trait with `HashedNgramEmbedder`.** Replaced Candidate 1's bare free function with a `pub(crate) trait Embedder` seam so Milestone 2 neural embeddings can replace local 256-dim trigram vectors without changing `VaultEngine` callers.
2. **Pre-heading `#_preamble` splitting and self-write consumption.** Preserved notes that start with prose before any ATX heading, and added consumption and pruning on `self_write_ledger` so reverting a file to an older daemon hash is never skipped forever.
3. **Empty-query `HOT` profile recall.** Allowed `recall` with an empty query when `include_hot_profile` is `true` so callers can assemble static system prompt prefixes on cold start.

Synthesis cleanup also branded `ContentHash` as a 64-hex newtype, standardized `CandidateId`, kept one `IgnoreRules` exclusion list shared across scan and inspect paths, and avoided storing redundant verbatim section copies in SQLite when `inspect_source` reads live Markdown from disk.

## Tradeoffs accepted

- **We accept a crate-local SQLite WAL schema inside `nytheris-aether-vault` in exchange for single-transaction ACID consistency** across AMUs, FTS5 indexes, wikilink edges, `self_write_ledger`, and `staged_candidates`.
- **We accept 256-dimensional signed trigram feature hashing as the default Milestone 1 `Embedder` in exchange for zero network or model dependencies**, preserving an internal trait boundary for Milestone 2 dense embeddings.
- **We accept `walkdir` plus SHA-256 polling instead of OS kernel watchers in exchange for strict offline compatibility with `Cargo.lock`**, because `notify` is not cached in `Cargo.lock`.
- **We accept re-reading the target Markdown file on disk during `aether_inspect_source` in exchange for immediate live accuracy and zero storage of `private` note bodies**, keeping `amus` rows lightweight.

## Alternatives considered

1. **Dual-database event pipeline over `SqliteRecordStore` and `cortex::Indexer` (Candidate 2).** Exposes callers to two SQLite files, requires translating `ScopeTag` onto incompatible `MemoryScope` keys, and splits ownership between `records` tombstones and `amu_meta` status fields. Lost on interface depth and single-owner consistency.
2. **Multi-actor channel pipeline (`Scanner`, `Parser`, `Indexer`, `Inbox` actors).** Organizes modules by execution phase instead of owned domain state, deepens call chains past three hops, and fragments crash recovery across channels. Lost due to temporal decomposition and split ownership.

## Implementation reconciliation

- `VaultEngine` exposes both `sync_vault` and `handle_fs_event` plus `reconcile_inbox` so file-watcher loop tests and Inbox approval tests can drive single-event and full-scan transitions directly while keeping internal store state private.

## Open questions and risks

1. Should approved `[x]` candidates without an explicit `target_note` remain solely in the compiled SQLite index or also append to `Aether/Compiled/Approved.md` so `aether_inspect_source` can cite an on-disk Markdown anchor?
2. Should Milestone 2 trigger automatic re-embedding on startup when `Embedder::dimensions()` differs from the vector blob size stored in SQLite?

## Next implementation step

Add `crates/aether-vault` to root `Cargo.toml` and write failing Red TDD tests for `types.rs` covering `SourcePointer` normalization, path traversal rejection, `AmuId` deterministic `MemoryId` mapping, and `ContentHash` validation.
