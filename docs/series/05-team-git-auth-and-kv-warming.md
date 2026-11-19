---
id: 05-team-git-auth-and-kv-warming
slug: /series/05-team-git-auth-and-kv-warming
sidebar_position: 5
---

# Part 5: Team Git Sync, Scoped Auth & GPU KV Warming

In this tutorial, you harden an Obsidian vault for multi-agent teams. You start `ControlPlaneServer::bind_with_auth`, issue per-agent `ScopedApiKey` credentials, synchronize a shared Git repository with `VaultEngine::git_sync`, connect `obsidian-aether-plugin`, and warm vLLM or SGLang KV caches.

**Before you begin.** Complete [Part 4: Obsidian Vault Compiler & MCP Server](./04-obsidian-vault-and-mcp.md). Initialize your vault directory as a Git repository with a remote named `origin` if you want to run `git_sync` end to end.

## Step 1: Start the authenticated team gateway

Generate a 64-hex bootstrap admin key (`aek_<64-hex>`) and start `aether-mcp` with `--require-auth`:

```bash
export AETHER_BOOTSTRAP_ADMIN_KEY="aek_$(openssl rand -hex 32)"

cargo run -p nytheris-aether-vault --bin aether-mcp -- \
  --vault /path/to/team-vault \
  --http-addr 0.0.0.0:4771 \
  --require-auth \
  --bootstrap-admin-key "$AETHER_BOOTSTRAP_ADMIN_KEY" \
  --kv-endpoint http://127.0.0.1:8000 \
  --kv-model llama-3.1-8b
```

Passing `--require-auth` allows non-loopback binds on `ControlPlaneServer::bind_with_auth` and enforces `Authorization: Bearer aek_<hex>` on every HTTP route. `ensure_bootstrap_admin_key` stores only the domain-separated SHA-256 digest of the secret in `agent_api_keys` under `AgentId` `"bootstrap-admin"`.

## Step 2: Issue and inspect scoped API keys

`AgentRole` defines three ordered roles (`Reader`, `Contributor`, `Admin`). Issue a `Contributor` key restricted to the `engineering` scope using the admin token:

```bash
curl -sS -X POST http://127.0.0.1:4771/v1/auth/keys \
  -H "Authorization: Bearer $AETHER_BOOTSTRAP_ADMIN_KEY" \
  -H 'Content-Type: application/json' \
  -d '{
    "agent_id": "coding-agent",
    "role": "contributor",
    "scopes": ["engineering"]
  }'
```

The JSON response returns `key` (`key_id` formatted as `key_<16-hex>`, `agent_id`, `role`, `scopes`, `created_at`) and a one-time plaintext `secret` (`aek_<64-hex>`). Store `secret` in `AETHER_API_KEY`.

To bind a `stdio` MCP client to that scoped agent, pass `--api-key "$AETHER_API_KEY"`:

```bash
cargo run -p nytheris-aether-vault --bin aether-mcp -- \
  --vault /path/to/team-vault \
  --api-key "$AETHER_API_KEY"
```

Under a scoped session:

- `aether_recall` intersects the requested `scope` with the key's `scopes` before top-K truncation. Keys with `*` may read all non-private scopes, and `global` notes are readable by any valid key.
- `aether_stage_memory` requires `Contributor` or `Admin`, defaults `scope` to `agent.default_write_scope()`, and stamps `proposed_by` with `agent.agent_id`.
- `aether_inspect_source` denies cross-scope notes and cross-scope entries inside `Aether/Compiled/Approved.md`.

List or revoke keys anytime with `GET /v1/auth/keys` and `POST /v1/auth/keys/revoke` (`Admin` only).

## Step 3: Synchronize a team Git vault (`git_sync`)

Run `POST /v1/git/sync` as an `Admin` (or call `VaultEngine::git_sync(GitSyncOptions::new())` in Rust):

```bash
curl -sS -X POST http://127.0.0.1:4771/v1/git/sync \
  -H "Authorization: Bearer $AETHER_BOOTSTRAP_ADMIN_KEY" \
  -H 'Content-Type: application/json' \
  -d '{"remote":"origin","branch":"main","push":true}'
```

During `git_sync`, `VaultEngine` executes five steps:

1. Ensures `.gitignore` ignores `.aether/` so local SQLite files never enter Git history.
2. Runs `git fetch` and merges into the local branch, redacting any `userinfo@` credentials from reported remotes via `redact_url_userinfo`.
3. Resolves merge conflicts in `Aether/Inbox.md` and `Aether/Compiled/Approved.md` automatically using a monotonic join-semilattice keyed on `CandidateId` where `Approved` overrides `Rejected`, and `Rejected` overrides `Pending`.
4. Skips human Markdown notes containing unresolved `<<<<<<<`, `=======`, and `>>>>>>>` conflict markers outside fenced code blocks while keeping prior SQLite AMUs active until a human resolves the conflict.
5. Re-syncs the vault, commits resolved state, and optionally pushes.

## Step 4: Connect `obsidian-aether-plugin` over HTTPS + bearer auth

When `obsidian-aether-plugin` targets a remote gateway, `AetherControlClient` and `AetherCompanionPlugin` require both `https://` and a bearer `apiKey` (unless `allowInsecureRemoteHttp: true` is set for local TLS-terminating proxies or tunnels):

```ts
import { AetherCompanionPlugin, type ObsidianHostAdapter } from './src/main.ts';

const plugin = new AetherCompanionPlugin(hostAdapter, 'https://vault.internal:4771', {
  apiKey: process.env.AETHER_API_KEY,
  defaultKvModel: 'llama-3.1-8b',
});
await plugin.onload();
```

`onload()` adds a status bar item, listens for live Obsidian vault events (`onVaultEvent`) when provided by `ObsidianHostAdapter`, and registers five Obsidian commands (`aether-open-inbox-review`, `aether-trigger-dream-pass`, `aether-sync-vault`, `aether-git-sync`, and `aether-warm-kv-cache`). Calling `plugin.syncVaultWithAether()` (or invoking `aether-sync-vault`) pulls `GET /v1/vault/pull`, pushes delta notes (`POST /v1/vault/push`) without ever transmitting `aether_tier: private` note bodies, and materializes updated `Aether/Inbox.md` and `Aether/Compiled/Approved.md` files back into Obsidian.

## Step 5: Pre-warm GPU KV caches (`warm_kv_cache`)

With `--kv-endpoint` and `--kv-model` configured (or using `VllmLmCacheHttpAdapter` or `InMemoryKvCacheAdapter` in Rust), warm PagedAttention prefix blocks and CacheBlend AMU chunks over HTTP (`Contributor` or `Admin`):

```bash
curl -sS -X POST http://127.0.0.1:4771/v1/kv/warm \
  -H "Authorization: Bearer $AETHER_API_KEY" \
  -H 'Content-Type: application/json' \
  -d '{
    "model_id": "llama-3.1-8b",
    "scope": "engineering",
    "target_tier": "gpu_hbm",
    "include_hot_prefix": true,
    "include_cache_blend": true
  }'
```

You receive a `KvWarmReport` with `blocks_warmed`, `blocks_reused`, `tokens_warmed`, `estimated_baseline_ttft_ms`, `estimated_warmed_ttft_ms`, `estimated_ttft_saved_ms`, `ttft_reduction_pct`, `fallback_used`, and `block_hashes`.

If the vLLM or LMCache server is unreachable and `offline_fallback` is `true` (the default on `VllmLmCacheConfig::new`), the adapter falls back to its local tiered `InMemoryKvCacheAdapter` (`GpuHbm` to `HostDram` to `NvmeSsd`) and sets `fallback_used: true`. Subsequent `aether_recall` calls passing `kv_model: "llama-3.1-8b"` attach `kv_hint` (`prefix_block_hashes`, `cache_blend_hashes`, `cache_hit_tokens`, `total_tokens`, `estimated_ttft_saved_ms`). Check live tier usage anytime via `GET /v1/kv/stats?model_id=llama-3.1-8b`.

## Where to go next

- Read [Concepts Overview: Model-Free & Pieces-First](../concepts/index.md) to learn how `RecordStore` consistency, `PersonalizedPageRank`, and KV block eviction work internally.
- Bookmark [Crates, Traits & API Reference](../reference/crates-and-apis.md) and [aether-mcp CLI, MCP Tools, HTTP API & Frontmatter](../reference/aether-mcp-and-http-api.md) for daily development lookup.
