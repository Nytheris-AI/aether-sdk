# Obsidian AMUs, Prefix Caching & KV-Tensor Warming

LLM inference becomes slow and expensive when agent prompts change ordering between turns or re-send stale notes. This page explains how `muninn-aether-vault` compiles Markdown sections into deterministic Aether Memory Units (AMUs), isolates private notes at the parse boundary, packs byte-stable prompt prefixes, and warms tiered GPU KV caches without letting deleted notes persist in GPU memory.

## Deterministic `SourcePointer` to `AmuId` and `MemoryId`

Every section inside a public Markdown note maps to a canonical `SourcePointer` (`Arch/DB.md#Schema`).

- `VaultPath::parse` normalizes backslashes to `/`, strips leading `./` segments, requires a `.md` suffix, and rejects empty paths, leading `/`, Windows drive prefixes, NUL bytes, `..` traversal, and system segments (`.obsidian`, `.git`, `.trash`, `.aether`).
- `parse_note` splits body text at ATX headings (`#` through `######`) outside fenced code blocks (` ``` ` and `~~~`). Prose before the first heading receives anchor `#_preamble`, and duplicate heading slugs in one file receive numeric suffixes (`-1`, `-2`).
- `AmuId::from_pointer` hashes `"aether:amu:v1:"` plus the canonical `SourcePointer` with SHA-256, sets RFC 4122 version and variant bits on the first 16 bytes, and wraps the resulting `MemoryId`.

Because derivation is deterministic and relies only on `sha2` (without requiring `uuid` `v5`), re-indexing an edited section keeps the same `AmuId` and `MemoryId` across machines and SQLite rebuilds.

## Parse-boundary isolation of `RawFrontmatterTier::Private`

Privacy cannot rely on query-time `WHERE` filters alone. In `crates/aether-vault/src/parser.rs`, `RawFrontmatterTier` contains `Public(VaultTier)` and `Private`, and stays `pub(crate)`. The public `VaultTier` enum in `types.rs` defines only `Hot`, `Warm`, and `Cold`.

When `aether_tier: private` is present in YAML frontmatter:

- `parse_note` returns `ParsedNote::Private` before extracting sections or embeddings.
- `VaultEngine::sync_vault` and `handle_fs_event` call `store.remove_file` and evict any cached KV blocks if the note had been public earlier.
- `Dream` consolidation skips the file before scanning lines.
- `inspect_source` checks `is_private_note` and returns `AetherError::InvalidInput`.

Because `Private` is unrepresentable in `VaultTier` and `StageMemoryRequest`, private content cannot enter `amus`, `amus_fts`, `Aether/Inbox.md`, or recall results.

## Canonical `[AETHER_HOT_PROFILE]` prefix packing

Provider prompt caches (Anthropic, OpenAI, vLLM, and SGLang) only hit when the prefix is byte-for-byte identical across requests.

`prefix::format_recall_response` enforces byte stability by layout:

1. Filter active `VaultTier::Hot` AMUs (respecting the active scope policy).
2. Sort strictly by `amu_id` ascending and deduplicate by `amu_id`.
3. Format each entry as `- [<amu_id>] (<source_pointer>) <distilled_fact>\n` under `[AETHER_HOT_PROFILE]\n`, omitting volatile wall-clock timestamps.
4. Enforce `HOT_PROFILE_MAX_TOKENS` (`2000`, estimated deterministically by `estimate_tokens` as `ceil(len / 4)`), `hot_budget`, and `max_tokens`.
5. Compute `prefix_hash` (`ContentHash::compute(hot_prefix.as_bytes())`), then pack ranked `Warm` and `Cold` AMUs under `[AETHER_RECALL_CONTEXT]\n` (`dynamic_context`) within the remaining `max_tokens` budget.

Calling `VaultEngine::recall` with an empty query string and `include_hot_profile: true` returns only `static_prefix` and its `prefix_hash`, allowing cold-start prompt warmup.

## PagedAttention prefix blocks and CacheBlend chunks

To bridge vault recall into local vLLM or SGLang inference engines, `kv.rs` maps the packed recall context into two complementary block hashing modes (`KvBlockMode`):

- **Chained prefix blocks (`KvBlockHash::paged_chained`).** Splits `static_prefix` into chunks of `block_tokens * 4` bytes (default `DEFAULT_KV_BLOCK_TOKENS = 16`, or `64` bytes per block) starting from `KvBlockHash::genesis()`. Each block hash is `SHA-256(model_id || 0x00 || parent_block_hash || 0x00 || block_text)`. Because hashes chain from block 0, any change at position `i` invalidates block `i` and its suffix while keeping earlier blocks intact.
- **Modular AMU chunks (`KvBlockHash::cache_blend`).** Maps each active `Warm` or `Cold` AMU independently via `SHA-256(model_id || 0x00 || amu_id || 0x00 || content_hash)`. Position-independent chunks support non-prefix KV reuse via CacheBlend even when query rankings reorder AMUs between turns.

## Tiered LRU eviction and TTFT estimation

`InMemoryKvCacheAdapter` (and the offline mirror inside `VllmLmCacheHttpAdapter`) models three storage tiers (`KvStorageTier`) ordered from fastest to slowest:

| Tier | Default capacity (`KvTierCapacity`) | Default hit cost (`KvTimingProfile`) | Eviction target |
| --- | --- | --- | --- |
| `GpuHbm` | `32,768` tokens | `2.0` ms per 1k tokens | Demotes LRU blocks to `HostDram` |
| `HostDram` | `262,144` tokens | `12.0` ms per 1k tokens | Demotes LRU blocks to `NvmeSsd` |
| `NvmeSsd` | `2,097,152` tokens | `35.0` ms per 1k tokens | Drops LRU blocks and deletes from SQLite |

Cold prefill baseline uses `120.0` ms per 1k tokens (`bytes_per_token = 65_536`). `VaultEngine::warm_kv_cache` and `attach_kv_hint` compute `estimated_baseline_ttft_ms`, `estimated_warmed_ttft_ms`, and `estimated_ttft_saved_ms` from live `KvCacheAdapter::lookup_blocks` hits.

## Transactional invalidation and `vault_meta` re-indexing

KV block metadata is persisted in SQLite (`kv_cache_blocks`) so `VaultEngine::open` rehydrates the active `KvCacheAdapter` on restart. Whenever `VaultStore` updates or removes a file, switches a note to `private`, or compiles a superseding Inbox approval:

1. `VaultStore` deletes affected `CacheBlendChunk` rows (`WHERE amu_id = ?1`) inside the same `IMMEDIATE` SQLite transaction.
2. If any modified or deleted AMU belonged to `VaultTier::Hot`, `VaultStore` also deletes `mode = 'paged_prefix'` rows so stale chained prefixes are never reused.
3. `VaultEngine` immediately passes all returned `KvBlockHash` values to `kv_adapter.evict_blocks(&evicted_kv)`, which notifies `VllmLmCacheHttpAdapter` (`POST /v1/kv/evict` under `LmCacheControl`).

The same single-database discipline governs embedder upgrades via `vault_meta` (`schema_version`, `embedder_id`, `embedding_dim`). When `VaultEngine::open` detects a changed embedder ID or dimension:

- If active AMU count is `<= sync_reindex_threshold` (`DEFAULT_SYNC_REINDEX_THRESHOLD = 5000`), `reembed_active_sync` re-computes all vectors in one `IMMEDIATE` transaction.
- Above threshold, `reembed_active_shadow_swap` computes new embeddings into `amus_shadow_embeddings` in chunks of `256` in the background and swaps them atomically while FTS5 and graph recall stay online.
