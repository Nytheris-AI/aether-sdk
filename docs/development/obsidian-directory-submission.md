# Submit AMC to the Obsidian directory

This page is the working checklist and the text to paste when submitting the plugin to the Obsidian community plugin directory.
Do the BRAT soak first, then submit.

## Which process to use

The current Obsidian docs ("Submit your plugin") describe submitting through `community.obsidian.md` with an Obsidian account that has your GitHub account linked.
The `obsidianmd/obsidian-releases` repository still contains `community-plugins.json`, and its README still points to the same docs page.
Follow the docs page you see on the day of submission, because the process has changed before.
If a pull request to `obsidianmd/obsidian-releases` is still requested, use the entry in [Legacy pull request entry](#legacy-pull-request-entry).

Two Obsidian pages could not be read when this checklist was written: "Developer policies" and "Submission requirements for plugins".
Read both on the day of submission and check the items they list against [Reviewer notes](#reviewer-notes).

## Pre-submission checklist

| Requirement | Status |
| --- | --- |
| Plugin repository is public with `README.md`, `LICENSE`, and `manifest.json` at the root | Done: `muninn-hq/aether-memory`, Apache-2.0 |
| `manifest.json` on the default branch matches the release version | Check: it must equal the release tag |
| GitHub release tag equals the manifest `version` (bare `x.y.z`) | Done for `0.4.1`; repeat for the release you submit |
| Release assets `main.js`, `manifest.json`, `styles.css` | Done: also `versions.json` |
| `id` is unique and does not contain `obsidian` | Done: `aether-memory`, not present in `community-plugins.json` |
| `description` is at most 250 characters and ends with a period | Done: enforced by a test |
| UI text in sentence case | Done in the sentence case change; release it before submitting |
| No global `app`, no console output, no inline styles, no `innerHTML`, no default hotkeys, no leaf detaching | Done: audited |
| Network calls use `requestUrl` | Done |
| `isDesktopOnly` matches reality | Done: `true` |
| Private-note data loss bug fixed | Done in `0.4.1` |
| BRAT soak with real vaults for several days | Open |

## Form values

| Field | Value |
| --- | --- |
| Plugin id | `aether-memory` |
| Name | `AMC` |
| Author | `Muninn Labs` |
| Repository | `muninn-hq/aether-memory` |
| Description | `Give your AI agents memory from your vault via MCP. Syncs notes to a local aether-mcp server and keeps private notes on disk.` |
| Minimum Obsidian version | `1.5.0` |
| Desktop only | Yes |

The name is short on purpose, because Obsidian prefixes every command in the palette with it ("AMC: Sync vault with Aether").
Directory search uses the name, author, and description, so the description keeps the words Aether, MCP, and memory.

## Reviewer notes

Paste this where the form asks about network use, external services, or anything reviewers should know.

**What the plugin does.**
It connects Obsidian Desktop to an `aether-mcp` server so AI agents that speak the Model Context Protocol can recall notes from the vault.
Notes sync automatically.
Facts an agent proposes wait in `Aether/Inbox.md` until you approve them in the inbox pane.

**Network use.**
The plugin sends requests only to the control-plane URL the user configures, through Obsidian's `requestUrl` API.
The default is `http://127.0.0.1:14771`, a server on the user's own machine.
A remote HTTPS endpoint is allowed with a bearer API key.
Plain HTTP to a non-loopback host is rejected.
The plugin sends no telemetry, crash reports, or third-party requests.

**External software.**
The plugin needs the open source `aether-mcp` server (Apache-2.0, `muninn-hq/aether-sdk`).
The settings tab explains this at the top and offers buttons that copy the install, service, and start commands to the clipboard.
The plugin never runs those commands itself and never spawns a process.

**What leaves the vault.**
Markdown notes are sent to the configured server, except notes that have `aether_tier: private` in frontmatter, paths matched by `.aetherignore`, and built-in exclusions (`.obsidian`, `.git`, `.trash`, `.aether`, and canvas or drawing files).
Private notes are filtered on the client before any request is built.
A server-side guard also refuses to delete a private note from disk.

**What the plugin writes.**
It writes `Aether/Inbox.md` and `Aether/Compiled/Approved.md` inside the vault, and its own settings in `data.json`.
The API key is stored in `data.json` in plain text, like other plugin settings.

**Optional feature.**
GPU KV-cache warming is off by default and aimed at people who run their own vLLM or SGLang server.
It only adds one command and two settings when enabled.

## Legacy pull request entry

If a pull request to `obsidianmd/obsidian-releases` is requested, append this object to the end of `community-plugins.json`.
The five fields and their order match the existing entries.

```json
{
  "id": "aether-memory",
  "name": "AMC",
  "author": "Muninn Labs",
  "description": "Give your AI agents memory from your vault via MCP. Syncs notes to a local aether-mcp server and keeps private notes on disk.",
  "repo": "muninn-hq/aether-memory"
}
```

## After approval

- Announce the first public release in the Obsidian forum showcase and in the `#updates` channel on the Obsidian Discord.
- Keep releasing from the monorepo tag; the sync workflow publishes the same files to `muninn-hq/aether-memory`.
- Respond to review feedback by publishing a new release with an incremented version, as the Obsidian docs require.
