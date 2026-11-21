---
sidebar_position: 4
---

# Share a vault with a team

Run `aether-mcp` as an authenticated gateway, give each agent its own scoped key, and keep a shared Git vault in sync.
For the Rust API behind these steps, see [Part 5 of the learning series](../series/05-team-git-auth-and-kv-warming.md).

## Start an authenticated server

```bash
export AETHER_BOOTSTRAP_ADMIN_KEY="aek_$(openssl rand -hex 32)"

aether-mcp \
  --vault /path/to/team-vault \
  --http-addr 0.0.0.0:4771 \
  --http-only \
  --require-auth \
  --bootstrap-admin-key "$AETHER_BOOTSTRAP_ADMIN_KEY"
```

`--require-auth` allows non-loopback binds and requires `Authorization: Bearer aek_<hex>` on every route.
Run the server behind HTTPS in production.
The Docker image in [Install aether-mcp](../get-started/install-aether-mcp.md) runs the same configuration.

## Issue a key per agent

Roles are ordered `Reader`, `Contributor`, `Admin`.
Create a `Contributor` key limited to the `engineering` scope:

```bash
curl -sS -X POST http://127.0.0.1:4771/v1/auth/keys \
  -H "Authorization: Bearer $AETHER_BOOTSTRAP_ADMIN_KEY" \
  -H 'Content-Type: application/json' \
  -d '{"agent_id":"coding-agent","role":"contributor","scopes":["engineering"]}'
```

The response includes a one-time `secret` (`aek_<64-hex>`).
Store it, because the server keeps only a hash.
List or revoke keys with `GET /v1/auth/keys` and `POST /v1/auth/keys/revoke` (`Admin` only).

Under a scoped key:

- `aether_recall` returns only notes in the key's scopes, plus `global` notes.
- `aether_stage_memory` needs `Contributor` or `Admin` and stamps the proposal with the agent id.
- `aether_inspect_source` denies notes outside the key's scopes.

## Connect AMC and agents

In **Settings**, **AMC**, set **Aether control-plane URL** to your `https://` gateway and paste a key into **API key**.
The connection card confirms the key works.
For `stdio` clients, add `"--api-key"` and the token to `args`, as described in [Connect an AI agent](../get-started/connect-an-agent.md).

AMC keeps the key in `.obsidian/plugins/aether-memory/data.json`, which Git sync keeps out of history.
Turn on **Run Dream consolidation on push** only with an `Admin` key.

## Sync the vault through Git

Run **Git sync team vault** in Obsidian, or call the endpoint as an `Admin`:

```bash
curl -sS -X POST http://127.0.0.1:4771/v1/git/sync \
  -H "Authorization: Bearer $AETHER_BOOTSTRAP_ADMIN_KEY" \
  -H 'Content-Type: application/json' \
  -d '{"remote":"origin","branch":"main","push":true}'
```

The sync ignores `.aether/` and plugin `data.json`, fetches and merges, resolves conflicts in `Aether/Inbox.md` and `Aether/Compiled/Approved.md` automatically (approved wins over rejected, rejected over pending), skips notes that still contain conflict markers, then commits and optionally pushes.
Your vault needs to be a Git repository with a remote such as `origin`.

## Optional: warm a GPU KV cache

If you run vLLM or SGLang, start `aether-mcp` with `--kv-endpoint` and `--kv-model`, turn on **Enable GPU KV-cache warming** in AMC, and run **Warm GPU KV cache**.
See [Part 5, step 5](../series/05-team-git-auth-and-kv-warming.md) for the request and response fields.
