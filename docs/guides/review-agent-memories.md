---
sidebar_position: 3
---

# Review what your agent wants to remember

Your own notes sync automatically.
Only facts an agent proposes wait for your approval before they become memory.

## How a proposal reaches you

1. The agent calls `aether_stage_memory`, which writes a candidate (`cand_<12-hex>`) to `Aether/Inbox.md`.
2. AMC pulls the update when you run **Sync vault with Aether** or after the **Auto-pull interval (seconds)**.
3. The status bar shows `Aether: N to review`.

## Approve or reject

Choose either way:

- Click the status bar item, or run **Open Aether inbox review**, then select **Approve** or **Reject** on each card.
- Open `Aether/Inbox.md`, change `[ ]` to `[x]` to approve or `[-]` to reject, and let **Live vault sync** push the file.

On approval, the server compiles the fact into active memory, adds a `## cand_<12-hex>` section to `Aether/Compiled/Approved.md`, removes it from the inbox, and syncs both files back to Obsidian.
Approved memories are then available to `aether_recall`.

## Find new candidates from daily notes

Run **Trigger dream pass** from the Command Palette.
AMC calls `POST /v1/dream`, which extracts candidate facts and flags contradictions from your Daily Notes, then shows how many notes it scanned, how many candidates it staged, and how many contradictions it flagged.

## Related

- [AMC walkthrough](./amc-walkthrough.md) shows the inbox in a real vault.
- [AMC plugin reference](../reference/amc-plugin.md) lists every command.
