---
sidebar_label: Vault files and frontmatter
---

# Vault files and frontmatter

How notes are tiered, scoped, and excluded.
For a task-oriented walkthrough, read [Organize and protect your notes](../guides/organize-and-protect-notes.md).

## YAML frontmatter keys

| Key | Accepted values | Default |
| --- | --- | --- |
| `aether_tier` | `hot`, `warm`, `cold`, `private` | `hot` if matched by `.aetherhot`, else `warm` |
| `aether_scope` | Scalar, comma-separated, `[a, b]`, or YAML list of `ScopeTag` | `global` |
| `aether_kind` | `profile`, `rule`, `decision`, `spec`, `fleeting` | `fleeting` under `Daily/` or `Journal/`, `spec` elsewhere |
| `aether_status` | `active`, `superseded`, `archived` | `active` |
| `valid_from` | RFC 3339 timestamp or `YYYY-MM-DD` date | Indexing timestamp |
| `superseded_by` | `amu_<32-hex>` or UUID | `None` |
| `tags` | Scalar, `[a, b]`, or YAML list of tags (`#` stripped) | Empty |

## Built-in exclusions, `.aetherignore`, and `.aetherhot`

Built-in rules skip paths containing `.obsidian`, `.git`, `.trash`, or `.aether` segments, prefixes `Aether/Inbox.md` and `Aether/Compiled` (except `Aether/Compiled/Approved.md` during `inspect_source`), and extensions `.canvas`, `.excalidraw`, and `.excalidraw.md`.

Place a `.aetherignore` file at the vault root to add newline-separated ignore rules. Blank lines and `#` comments are ignored. Trailing `/` matches directories, `*` and `?` match globs, and paths with `/` match from the vault root.

Place a `.aetherhot` file at the vault root using the same path/glob syntax to promote matching notes into `hot` (`[AETHER_HOT_PROFILE]`) when YAML frontmatter does not specify an explicit `aether_tier`. Explicit frontmatter (`private`, `warm`, `cold`, `hot`) always takes precedence.
