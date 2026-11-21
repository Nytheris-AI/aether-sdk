---
sidebar_position: 2
---

# Organize and protect your notes

Frontmatter and two small files at the vault root control what your agent sees, how important each note is, and what never leaves your disk.

## Frontmatter

Add YAML frontmatter to any note:

- `aether_tier` accepts `hot`, `warm`, `cold`, or `private` (default `warm`).
  Notes marked `hot` load into `[AETHER_HOT_PROFILE]` on every recall.
  Notes marked `private` never leave your vault.
- `aether_scope` takes one scope, a comma-separated list, `[a, b]`, or a YAML list (default `global`).
- `aether_kind` is one of `profile`, `rule`, `decision`, `spec`, or `fleeting`.

## Keep a note private

```markdown
---
aether_tier: private
---
# Personal keys
Local credentials that stay on disk only.
```

AMC filters private notes on your device before it builds any request, and the server refuses to read or delete them.

To check it, run **Sync vault with Aether** and confirm the notice reports private notes kept on your device.
You can also call `POST /v1/inspect` or `aether_inspect_source` with the note path and confirm the server rejects it.

## Exclude folders with `.aetherignore`

Create `.aetherignore` at the vault root to skip whole folders or filename patterns:

```text
Finance/
*.secret.md
```

Built-in rules already skip `.obsidian`, `.git`, `.trash`, `.aether`, `Aether/Inbox.md`, `Aether/Compiled`, `.canvas`, `.excalidraw`, and `.excalidraw.md`.

## Pin notes into the hot profile with `.aetherhot`

To load notes into `[AETHER_HOT_PROFILE]` without editing their frontmatter, create `.aetherhot` with the same syntax:

```text
Memory/
Profile/*.md
```

Matched notes become `hot` unless their frontmatter sets an explicit `aether_tier`.

## Reference

Every key, default, and rule is in [Vault files and frontmatter](../reference/vault-files.md).
