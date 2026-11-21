---
sidebar_position: 1
---

# Quickstart

Get an AI agent to recall your Obsidian notes in about five minutes.
You need Obsidian Desktop 1.5.0 or later and an MCP client such as Cursor, Claude Desktop, Claude Code, or Antigravity.
You do not need Rust or Node.js.

1. **Install `aether-mcp` and start it for your vault.**

   ```bash
   curl -fsSL https://raw.githubusercontent.com/muninn-hq/aether-sdk/main/scripts/install.sh | sh
   aether-mcp service install --vault /path/to/vault
   ```

   On Windows, use the PowerShell installer in [Install aether-mcp](./install-aether-mcp.md).

2. **Install AMC in Obsidian.**
   Install **Obsidian42 BRAT**, choose **Add beta plugin**, and enter `https://github.com/muninn-hq/aether-memory`.
   Then enable **AMC** under **Settings**, **Community plugins**.
   Details are in [Install AMC](./install-amc.md).

3. **Check the connection.**
   Open **Settings**, then **AMC**.
   The card at the top should say **Connected to aether-mcp**.

4. **Connect your agent.**
   Add this to your MCP client configuration:

   ```json
   {
     "mcpServers": {
       "aether-vault": {
         "command": "aether-mcp",
         "args": ["--vault", "/path/to/vault", "--watch-ms", "0"]
       }
     }
   }
   ```

5. **Ask your agent about a note.**
   Ask something your vault answers, such as "What did I decide about the database schema?".
   The agent calls `aether_recall` and answers from your notes.

Notes sync automatically.
Anything marked `aether_tier: private` in its frontmatter stays on your disk.

## Where to go next

- Watch the [AMC walkthrough](../guides/amc-walkthrough.md) to see each step in a real vault.
- Learn how to [organize and protect your notes](../guides/organize-and-protect-notes.md).
- Learn how to [review what your agent wants to remember](../guides/review-agent-memories.md).
- If something does not work, see [Troubleshooting](../guides/troubleshooting.md).
