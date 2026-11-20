import assert from 'node:assert/strict';
import fs from 'node:fs';
import http from 'node:http';
import os from 'node:os';
import path from 'node:path';
import {describe, it} from 'node:test';
import {fileURLToPath} from 'node:url';
import {
  REQUIRED_RELEASE_FILES,
  formatVerificationReport,
  parseCliArgs,
  runReleaseCli,
  validateRelease,
  verifyReleaseDirectory,
} from '../scripts/verify-release.mjs';
import {AetherControlClient, assertControlPlaneBaseUrl, assertLoopbackBaseUrl} from '../src/api.ts';
import {
  ObsidianDesktopAdapter,
  PathDebouncer,
  createRequestUrlFetch,
} from '../src/desktop-adapter.ts';
import {
  AetherCompanionPlugin,
  computeSha256Hex,
  formatStatusBarText,
  isIgnoredVaultPath,
  isPrivateMarkdownNote,
  type ObsidianCommandDescriptor,
  type ObsidianHostAdapter,
} from '../src/main.ts';
import {
  buildLocalStartCommand,
  buildServiceInstallCommand,
  DEFAULT_SETTINGS,
  normalizePluginSettings,
} from '../src/settings-model.ts';
import type {InboxEntry, VaultEvent, VaultStatusReport} from '../src/types.ts';

function createMockHost() {
  const statusTexts: string[] = [];
  const commands = new Map<string, ObsidianCommandDescriptor>();
  const notices: string[] = [];
  let openedPaneCount = 0;

  const host: ObsidianHostAdapter = {
    addStatusBarItem() {
      return {
        setText(text: string) {
          statusTexts.push(text);
        },
      };
    },
    addCommand(cmd) {
      commands.set(cmd.id, cmd);
    },
    notify(message) {
      notices.push(message);
    },
    openInboxPane() {
      openedPaneCount += 1;
    },
  };

  return {
    host,
    statusTexts,
    commands,
    notices,
    getOpenedPaneCount: () => openedPaneCount,
  };
}

describe('Aether Obsidian Companion Plugin', () => {
  it('defaults control-plane endpoint URL to loopback port 14771 across client, plugin, and settings', () => {
    const mock = createMockHost();
    assert.equal(DEFAULT_SETTINGS.endpointUrl, 'http://127.0.0.1:14771');
    assert.equal(normalizePluginSettings(null).endpointUrl, 'http://127.0.0.1:14771');
    assert.equal(
      normalizePluginSettings({endpointUrl: '   '}).endpointUrl,
      'http://127.0.0.1:14771',
    );
    assert.equal(
      normalizePluginSettings({endpointUrl: 'http://127.0.0.1:3000'}).endpointUrl,
      'http://127.0.0.1:3000',
    );
    assert.equal(new AetherControlClient().baseUrl, 'http://127.0.0.1:14771');
    assert.equal(new AetherCompanionPlugin(mock.host).client.baseUrl, 'http://127.0.0.1:14771');
  });

  it('enforces loopback URLs and rejects remote hosts', () => {
    assert.equal(assertLoopbackBaseUrl('http://127.0.0.1:14771'), 'http://127.0.0.1:14771');
    assert.equal(assertLoopbackBaseUrl('http://localhost:14771'), 'http://localhost:14771');
    assert.throws(() => assertLoopbackBaseUrl('http://192.168.1.50:14771'));
    assert.throws(() => assertLoopbackBaseUrl('https://127.0.0.1:14771'));
    assert.throws(() => new AetherControlClient('http://example.com:14771'));
  });

  it('formats status bar indicator and drives 1-click Inbox approval and Dream command over HTTP', async () => {
    const statusState: VaultStatusReport = {
      status: 'ok',
      schema_version: 2,
      embedder_id: 'hashed-ngram-256-v1',
      embedding_dim: 256,
      active_amus: 4,
      pending_inbox_count: 0,
      approved_inbox_count: 0,
      hot_tokens_used: 120,
      hot_token_budget: 2000,
      prefix_hash: '0'.repeat(64),
    };
    let pending: InboxEntry[] = [];

    const server = http.createServer((req, res) => {
      let body = '';
      req.on('data', (chunk) => {
        body += chunk;
      });
      req.on('end', () => {
        res.setHeader('Content-Type', 'application/json');
        if (req.method === 'GET' && req.url === '/v1/status') {
          res.end(JSON.stringify(statusState));
          return;
        }
        if (req.method === 'GET' && req.url === '/v1/inbox') {
          res.end(
            JSON.stringify({
              inbox_path: 'Aether/Inbox.md',
              approved_mirror_path: 'Aether/Compiled/Approved.md',
              pending,
            }),
          );
          return;
        }
        if (req.method === 'POST' && req.url === '/v1/dream') {
          pending = [
            {
              checkbox: 'pending',
              tier: 'warm',
              scope: 'engineering',
              proposed_fact: 'Use LanceDB for zero-copy vector storage.',
              candidate_id: 'cand_123456abcdef',
              target_link: 'Arch/Vector',
              proposed_by: 'dream-worker',
              proposed_date: '2026-05-10',
              supersedes_amu_id: null,
            },
          ];
          statusState.pending_inbox_count = 1;
          res.end(
            JSON.stringify({
              daily_notes_scanned: 1,
              daily_notes_skipped_unchanged: 0,
              candidates_staged: 1,
              contradictions_flagged: 0,
              staged_candidate_ids: ['cand_123456abcdef'],
            }),
          );
          return;
        }
        if (req.method === 'POST' && req.url === '/v1/inbox/decide') {
          const parsed = JSON.parse(body) as {candidate_id: string; decision: string};
          pending = pending.filter((p) => p.candidate_id !== parsed.candidate_id);
          statusState.pending_inbox_count = pending.length;
          if (parsed.decision === 'approve') {
            statusState.active_amus += 1;
            statusState.approved_inbox_count += 1;
          }
          res.end(
            JSON.stringify({
              candidate_id: parsed.candidate_id,
              decision: parsed.decision,
              reconcile: {compiled: 1, rejected: 0, remaining_pending: pending.length},
            }),
          );
          return;
        }
        res.statusCode = 404;
        res.end(JSON.stringify({error: 'not found'}));
      });
    });

    await new Promise<void>((resolve) => {
      server.listen(0, '127.0.0.1', () => resolve());
    });

    try {
      const addr = server.address();
      assert.ok(addr && typeof addr === 'object');
      const baseUrl = `http://127.0.0.1:${addr.port}`;
      const mock = createMockHost();
      const plugin = new AetherCompanionPlugin(mock.host, baseUrl);
      await plugin.onload();

      assert.equal(mock.statusTexts.at(-1), 'Aether: 4 AMUs | 0 Inbox | HOT 120/2000');
      assert.equal(formatStatusBarText(null), 'Aether: Offline');

      const dreamCmd = mock.commands.get('aether-trigger-dream-pass');
      assert.ok(dreamCmd);
      await dreamCmd.callback();

      assert.equal(plugin.inboxController.pending.length, 1);
      assert.equal(mock.statusTexts.at(-1), 'Aether: 4 AMUs | 1 Inbox | HOT 120/2000');
      assert.ok(mock.notices.at(-1)?.includes('staged 1 candidate(s)'));

      const reviewCmd = mock.commands.get('aether-open-inbox-review');
      assert.ok(reviewCmd);
      await reviewCmd.callback();
      assert.equal(mock.getOpenedPaneCount(), 1);

      await plugin.inboxController.decide('cand_123456abcdef', 'approve');
      assert.equal(plugin.inboxController.pending.length, 0);
      assert.equal(mock.statusTexts.at(-1), 'Aether: 5 AMUs | 0 Inbox | HOT 120/2000');
    } finally {
      await new Promise<void>((resolve, reject) => {
        server.close((err) => (err ? reject(err) : resolve()));
      });
    }
  });

  it('enforces remote bearer auth policy and drives Git sync and KV warming commands', async () => {
    assert.equal(
      assertControlPlaneBaseUrl('https://team-vault.internal:14771', {apiKey: 'aek_test1234'}),
      'https://team-vault.internal:14771',
    );
    assert.throws(() => assertControlPlaneBaseUrl('https://team-vault.internal:14771'));
    assert.throws(() =>
      assertControlPlaneBaseUrl('http://192.168.1.50:14771', {apiKey: 'aek_test1234'}),
    );
    assert.equal(
      assertControlPlaneBaseUrl('http://192.168.1.50:14771', {
        apiKey: 'aek_test1234',
        allowInsecureRemoteHttp: true,
      }),
      'http://192.168.1.50:14771',
    );

    const statusState: VaultStatusReport = {
      status: 'ok',
      schema_version: 3,
      embedder_id: 'hashed-ngram-256-v1',
      embedding_dim: 256,
      active_amus: 6,
      pending_inbox_count: 0,
      approved_inbox_count: 2,
      hot_tokens_used: 180,
      hot_token_budget: 2000,
      prefix_hash: 'a'.repeat(64),
    };
    const seenAuthHeaders: Array<string | undefined> = [];

    const server = http.createServer((req, res) => {
      seenAuthHeaders.push(req.headers.authorization);
      let body = '';
      req.on('data', (chunk) => {
        body += chunk;
      });
      req.on('end', () => {
        res.setHeader('Content-Type', 'application/json');
        if (req.headers.authorization !== 'Bearer aek_team_secret_0001') {
          res.statusCode = 401;
          res.end(JSON.stringify({error: 'unauthorized'}));
          return;
        }
        if (req.method === 'GET' && req.url === '/v1/status') {
          res.end(JSON.stringify(statusState));
          return;
        }
        if (req.method === 'GET' && req.url === '/v1/inbox') {
          res.end(
            JSON.stringify({
              inbox_path: 'Aether/Inbox.md',
              approved_mirror_path: 'Aether/Compiled/Approved.md',
              pending: [],
            }),
          );
          return;
        }
        if (req.method === 'POST' && req.url === '/v1/git/sync') {
          statusState.active_amus = 8;
          res.end(
            JSON.stringify({
              remote: 'https://git.example.com/team/vault.git',
              branch: 'main',
              head_commit: 'abc1234',
              merged_cleanly: true,
              aether_conflicts_resolved: 2,
              conflicted_files: [],
              pushed: true,
              sync_report: {
                files_scanned: 5,
                files_skipped_unchanged: 2,
                files_skipped_self_write: 0,
                files_updated: 3,
                files_removed: 0,
                private_skipped: 0,
                amus_indexed: 8,
                inbox_approved: 2,
                inbox_rejected: 0,
                reindexed_amus: 0,
              },
            }),
          );
          return;
        }
        if (req.method === 'POST' && req.url === '/v1/kv/warm') {
          const parsed = JSON.parse(body) as {model_id: string};
          res.end(
            JSON.stringify({
              model_id: parsed.model_id,
              target_tier: 'gpu_hbm',
              blocks_warmed: 4,
              blocks_reused: 2,
              tokens_warmed: 512,
              estimated_baseline_ttft_ms: 92.0,
              estimated_warmed_ttft_ms: 8.5,
              estimated_ttft_saved_ms: 83.5,
              ttft_reduction_pct: 90.8,
              fallback_used: false,
              block_hashes: ['b'.repeat(64)],
            }),
          );
          return;
        }
        if (req.method === 'GET' && req.url === '/v1/kv/stats?model_id=qwen2.5-coder') {
          res.end(
            JSON.stringify({
              adapter_id: 'in-memory-kv-v1',
              persisted_blocks: 4,
              total_tokens: 512,
              total_bytes: 33554432,
              tier_usage: {
                gpu_hbm_blocks: 4,
                host_dram_blocks: 0,
                nvme_ssd_blocks: 0,
                gpu_hbm_tokens: 512,
                host_dram_tokens: 0,
                nvme_ssd_tokens: 0,
              },
              models: ['qwen2.5-coder'],
            }),
          );
          return;
        }
        res.statusCode = 404;
        res.end(JSON.stringify({error: 'not found'}));
      });
    });

    await new Promise<void>((resolve) => {
      server.listen(0, '127.0.0.1', () => resolve());
    });

    try {
      const addr = server.address();
      assert.ok(addr && typeof addr === 'object');
      const baseUrl = `http://127.0.0.1:${addr.port}`;
      const mock = createMockHost();
      const plugin = new AetherCompanionPlugin(mock.host, baseUrl, {
        apiKey: 'aek_team_secret_0001',
        defaultKvModel: 'qwen2.5-coder',
      });
      await plugin.onload();

      const gitSyncCmd = mock.commands.get('aether-git-sync');
      assert.ok(gitSyncCmd);
      await gitSyncCmd.callback();
      assert.equal(mock.statusTexts.at(-1), 'Aether: 8 AMUs | 0 Inbox | HOT 180/2000');
      assert.ok(mock.notices.at(-1)?.includes('resolved 2 Aether conflict(s)'));

      const kvWarmCmd = mock.commands.get('aether-warm-kv-cache');
      assert.ok(kvWarmCmd);
      await kvWarmCmd.callback();
      assert.ok(mock.notices.at(-1)?.includes('qwen2.5-coder'));
      assert.ok(mock.notices.at(-1)?.includes('90.8%'));

      const stats = await plugin.client.getKvStats('qwen2.5-coder');
      assert.equal(stats.persisted_blocks, 4);
      assert.ok(seenAuthHeaders.every((hdr) => hdr === 'Bearer aek_team_secret_0001'));
    } finally {
      await new Promise<void>((resolve, reject) => {
        server.close((err) => (err ? reject(err) : resolve()));
      });
    }
  });

  it('filters private and ignored paths client-side and hashes consistently', async () => {
    assert.equal(
      isPrivateMarkdownNote('---\naether_tier: private\n---\n# Secret\nNever leave disk.'),
      true,
    );
    assert.equal(
      isPrivateMarkdownNote('---\naether_tier: "Private"\n---\n# Secret'),
      true,
    );
    assert.equal(
      isPrivateMarkdownNote('---\naether_tier: warm\n---\n# Public\naether_tier: private'),
      false,
    );

    const ignoreFile = '# user ignores\nFinance/\n*.secret.md\n';
    assert.equal(isIgnoredVaultPath('.obsidian/workspace.md', ignoreFile), true);
    assert.equal(isIgnoredVaultPath('.obsidian/plugins/aether-memory/data.json'), true);
    assert.equal(isIgnoredVaultPath('.obsidian/plugins/aether-memory/data.json', ignoreFile), true);
    assert.equal(isIgnoredVaultPath('.git/HEAD', ignoreFile), true);
    assert.equal(isIgnoredVaultPath('Aether/Inbox.md', ignoreFile), true);
    assert.equal(isIgnoredVaultPath('Aether/Compiled/Approved.md', ignoreFile), true);
    assert.equal(isIgnoredVaultPath('Diagrams/flow.excalidraw.md', ignoreFile), true);
    assert.equal(isIgnoredVaultPath('Assets/logo.png', ignoreFile), true);
    assert.equal(isIgnoredVaultPath('Finance/2026.md', ignoreFile), true);
    assert.equal(isIgnoredVaultPath('Personal/keys.secret.md', ignoreFile), true);
    assert.equal(isIgnoredVaultPath('Arch/DB.md', ignoreFile), false);

    const h1 = await computeSha256Hex('aether');
    const h2 = await computeSha256Hex('aether');
    assert.equal(h1.length, 64);
    assert.equal(h1, h2);
  });

  it('syncs delta notes, suppresses private bodies, handles real-time events, and writes Inbox/Approved mirrors back', async () => {
    const files = new Map<string, string>([
      [
        'Arch/DB.md',
        '---\naether_tier: warm\naether_scope:\n  - engineering\n---\n# DB\nUse SQLite WAL.\n',
      ],
      [
        'Personal/Diary.md',
        '---\naether_tier: private\n---\n# Diary\nCLIENT_ONLY_SECRET_TOKEN\n',
      ],
      ['Finance/Payroll.md', '# Payroll\nPrivate payroll data.\n'],
      ['.aetherignore', 'Finance/\n'],
    ]);

    const vaultEventHandlers: Array<(event: VaultEvent) => Promise<void>> = [];
    const statusTexts: string[] = [];
    const commands = new Map<string, ObsidianCommandDescriptor>();
    const notices: string[] = [];
    const writeCounts = new Map<string, number>();

    const host: ObsidianHostAdapter = {
      addStatusBarItem() {
        return {
          setText(text: string) {
            statusTexts.push(text);
          },
        };
      },
      addCommand(cmd) {
        commands.set(cmd.id, cmd);
      },
      notify(message) {
        notices.push(message);
      },
      openInboxPane() {},
      async listMarkdownFiles() {
        return [...files.keys()].filter((p) => p.toLowerCase().endsWith('.md'));
      },
      async readFile(path) {
        return files.get(path) ?? null;
      },
      async writeFile(path, content) {
        files.set(path, content);
        writeCounts.set(path, (writeCounts.get(path) ?? 0) + 1);
      },
      onVaultEvent(handler) {
        vaultEventHandlers.push(handler);
      },
    };

    const indexedNotes = new Map<string, string>();
    const pushedPayloads: Array<{raw: string; path: string}> = [];
    let pendingInbox: InboxEntry[] = [
      {
        checkbox: 'pending',
        tier: 'warm',
        scope: 'engineering',
        proposed_fact: 'Always run cargo fmt before commit.',
        candidate_id: 'cand_111122223333',
        target_link: 'Arch/DB',
        proposed_by: 'mcp-agent',
        proposed_date: '2026-11-19',
        supersedes_amu_id: null,
      },
    ];
    let approvedMarkdown = '';

    const renderInbox = () =>
      [
        '# Aether Memory Inbox',
        '> Check `[x]` to approve and compile into active vault memory. Delete or mark `[-]` to reject.',
        ...pendingInbox.map(
          (e) =>
            `\n- [ ] **[${e.tier.toUpperCase()} | ${e.scope}]** ${e.proposed_fact}\n  - \`amu_candidate_id\`: \`${e.candidate_id}\``,
        ),
      ].join('\n');

    const server = http.createServer(async (req, res) => {
      let body = '';
      for await (const chunk of req) {
        body += chunk;
      }
      res.setHeader('Content-Type', 'application/json');
      if (req.method === 'GET' && req.url === '/v1/status') {
        res.end(
          JSON.stringify({
            status: 'ok',
            schema_version: 3,
            embedder_id: 'hashed-ngram-256-v1',
            embedding_dim: 256,
            active_amus: indexedNotes.size + (approvedMarkdown ? 1 : 0),
            pending_inbox_count: pendingInbox.length,
            approved_inbox_count: approvedMarkdown ? 1 : 0,
            hot_tokens_used: 0,
            hot_token_budget: 2000,
            prefix_hash: '0'.repeat(64),
          }),
        );
        return;
      }
      if (req.method === 'GET' && req.url === '/v1/inbox') {
        res.end(
          JSON.stringify({
            inbox_path: 'Aether/Inbox.md',
            approved_mirror_path: 'Aether/Compiled/Approved.md',
            pending: pendingInbox,
          }),
        );
        return;
      }
      if (req.method === 'GET' && req.url === '/v1/vault/pull') {
        const manifest: Record<string, string> = {};
        for (const [k, v] of indexedNotes.entries()) {
          manifest[k] = await computeSha256Hex(v);
        }
        res.end(
          JSON.stringify({
            manifest,
            inbox_path: 'Aether/Inbox.md',
            inbox_markdown: renderInbox(),
            approved_mirror_path: 'Aether/Compiled/Approved.md',
            approved_mirror_markdown: approvedMarkdown,
          }),
        );
        return;
      }
      if (req.method === 'POST' && req.url === '/v1/vault/push') {
        pushedPayloads.push({raw: body, path: req.url});
        const parsed = JSON.parse(body) as {
          notes?: Array<{path: string; content: string}>;
          deleted_paths?: string[];
          private_paths?: string[];
          inbox_markdown?: string;
        };
        let filesUpdated = 0;
        let filesRemoved = 0;
        let privateSkipped = parsed.private_paths?.length ?? 0;
        let inboxApproved = 0;

        for (const p of parsed.private_paths ?? []) {
          indexedNotes.delete(p);
        }
        for (const p of parsed.deleted_paths ?? []) {
          if (indexedNotes.delete(p)) {
            filesRemoved += 1;
          }
        }
        for (const n of parsed.notes ?? []) {
          indexedNotes.set(n.path, n.content);
          filesUpdated += 1;
        }
        if (parsed.inbox_markdown?.includes('- [x] ')) {
          inboxApproved = pendingInbox.length;
          pendingInbox = [];
          approvedMarkdown =
            '# Aether Compiled Approved Memory\n\n## cand_111122223333\nAlways run cargo fmt before commit.\n';
        }
        const syncReport = {
          files_scanned: indexedNotes.size,
          files_skipped_unchanged: 0,
          files_skipped_self_write: 0,
          files_updated: filesUpdated,
          files_removed: filesRemoved,
          private_skipped: privateSkipped,
          amus_indexed: filesUpdated,
          inbox_approved: inboxApproved,
          inbox_rejected: 0,
          reindexed_amus: 0,
        };
        res.end(
          JSON.stringify({
            sync_report: syncReport,
            inbox_path: 'Aether/Inbox.md',
            inbox_markdown: renderInbox(),
            approved_mirror_path: 'Aether/Compiled/Approved.md',
            approved_mirror_markdown: approvedMarkdown,
          }),
        );
        return;
      }
      res.statusCode = 404;
      res.end(JSON.stringify({error: 'not found'}));
    });

    await new Promise<void>((resolve) => {
      server.listen(0, '127.0.0.1', () => resolve());
    });

    try {
      const addr = server.address();
      assert.ok(addr && typeof addr === 'object');
      const plugin = new AetherCompanionPlugin(host, `http://127.0.0.1:${addr.port}`);
      await plugin.onload();

      const syncCmd = commands.get('aether-sync-vault');
      assert.ok(syncCmd);
      await syncCmd.callback();

      assert.equal(indexedNotes.has('Arch/DB.md'), true);
      assert.equal(indexedNotes.has('Personal/Diary.md'), false);
      assert.equal(indexedNotes.has('Finance/Payroll.md'), false);
      assert.equal(
        pushedPayloads.some((p) => p.raw.includes('CLIENT_ONLY_SECRET_TOKEN')),
        false,
      );
      assert.ok(files.get('Aether/Inbox.md')?.includes('cand_111122223333'));
      assert.equal(
        notices.at(-1),
        'Aether vault sync: updated 1 file(s), indexed 1 AMU(s), removed 0 file(s), skipped 1 private.',
      );

      const emitVaultEvent = vaultEventHandlers[0];
      assert.ok(emitVaultEvent);
      const writesBeforeSelfCheck = writeCounts.get('Aether/Inbox.md') ?? 0;
      const pushesBeforeSelfCheck = pushedPayloads.length;
      await emitVaultEvent({kind: 'modify', path: 'Aether/Inbox.md'});
      assert.equal(pushedPayloads.length, pushesBeforeSelfCheck);
      assert.equal(writeCounts.get('Aether/Inbox.md'), writesBeforeSelfCheck);

      files.set('Arch/NewSpec.md', '# NewSpec\nUse deterministic prefix hashes.\n');
      await emitVaultEvent({kind: 'create', path: 'Arch/NewSpec.md'});
      assert.equal(indexedNotes.has('Arch/NewSpec.md'), true);

      files.delete('Arch/NewSpec.md');
      files.set('Arch/PrefixSpec.md', '# PrefixSpec\nUse deterministic prefix hashes.\n');
      await emitVaultEvent({
        kind: 'rename',
        oldPath: 'Arch/NewSpec.md',
        newPath: 'Arch/PrefixSpec.md',
      });
      assert.equal(indexedNotes.has('Arch/NewSpec.md'), false);
      assert.equal(indexedNotes.has('Arch/PrefixSpec.md'), true);

      files.set('Arch/DB.md', '---\naether_tier: private\n---\n# DB\nNOW_PRIVATE_BODY\n');
      await emitVaultEvent({kind: 'modify', path: 'Arch/DB.md'});
      assert.equal(indexedNotes.has('Arch/DB.md'), false);
      const lastPush = JSON.parse(pushedPayloads.at(-1)!.raw) as {
        private_paths?: string[];
        deleted_paths?: string[];
        notes?: unknown[];
      };
      assert.deepEqual(lastPush.private_paths, ['Arch/DB.md']);
      assert.deepEqual(lastPush.deleted_paths, ['Arch/DB.md']);
      assert.equal(lastPush.notes?.length ?? 0, 0);
      assert.equal(
        pushedPayloads.some((p) => p.raw.includes('NOW_PRIVATE_BODY')),
        false,
      );

      files.set(
        'Aether/Inbox.md',
        files.get('Aether/Inbox.md')!.replace('- [ ] ', '- [x] '),
      );
      await emitVaultEvent({kind: 'modify', path: 'Aether/Inbox.md'});
      assert.ok(files.get('Aether/Compiled/Approved.md')?.includes('cand_111122223333'));
      assert.equal(files.get('Aether/Inbox.md')?.includes('cand_111122223333'), false);

      files.delete('Arch/PrefixSpec.md');
      await emitVaultEvent({kind: 'delete', path: 'Arch/PrefixSpec.md'});
      assert.equal(indexedNotes.has('Arch/PrefixSpec.md'), false);
      assert.ok(statusTexts.length > 0);
    } finally {
      await new Promise<void>((resolve, reject) => {
        server.close((err) => (err ? reject(err) : resolve()));
      });
    }
  });

  it('desktop adapter gates events on onLayoutReady, creates parent folders, uses vault.read, and debounces safely', async () => {
    const folders = new Set<string>();
    const createdFolders: string[] = [];
    const files = new Map<string, string>([['Arch/DB.md', '# DB\nInitial WAL text.\n']]);
    const listeners = new Map<string, Array<(...args: unknown[]) => void>>();
    const registeredEvents: unknown[] = [];
    let layoutReadyCallback: (() => void) | null = null;
    let readCallCount = 0;
    let cachedReadCallCount = 0;

    const makeFileObj = (path: string) => ({
      path,
      name: path.split('/').pop() ?? path,
      extension: path.split('.').pop() ?? '',
    });
    const makeFolderObj = (path: string) => ({
      path,
      name: path.split('/').pop() ?? path,
      children: [],
    });

    const emitObsidian = (name: string, ...args: unknown[]) => {
      for (const cb of listeners.get(name) ?? []) {
        cb(...args);
      }
    };

    const mockApp = {
      vault: {
        getMarkdownFiles() {
          return [...files.keys()]
            .filter((p) => p.toLowerCase().endsWith('.md'))
            .map((p) => makeFileObj(p));
        },
        getAbstractFileByPath(path: string) {
          if (files.has(path)) {
            return makeFileObj(path);
          }
          if (folders.has(path)) {
            return makeFolderObj(path);
          }
          return null;
        },
        async read(file: {path: string}) {
          readCallCount += 1;
          return files.get(file.path) ?? '';
        },
        async cachedRead(file: {path: string}) {
          cachedReadCallCount += 1;
          return files.get(file.path) ?? '';
        },
        async createFolder(path: string) {
          folders.add(path);
          createdFolders.push(path);
        },
        async create(path: string, data: string) {
          files.set(path, data);
          return makeFileObj(path);
        },
        async modify(file: {path: string}, data: string) {
          files.set(file.path, data);
        },
        on(name: string, cb: (...args: unknown[]) => void) {
          const list = listeners.get(name) ?? [];
          list.push(cb);
          listeners.set(name, list);
          return {name, cb};
        },
      },
      workspace: {
        onLayoutReady(cb: () => void) {
          layoutReadyCallback = cb;
        },
      },
    };

    const adapter = new ObsidianDesktopAdapter({
      app: mockApp as never,
      registerEvent(ref) {
        registeredEvents.push(ref);
      },
      addStatusBarItem() {
        return {setText() {}};
      },
      addCommand() {},
      notify() {},
      openInboxPane() {},
      debounceMs: 25,
      isLiveSyncEnabled: () => true,
    });

    const content = await adapter.readFile('./Arch/DB.md');
    assert.equal(content, '# DB\nInitial WAL text.\n');
    assert.equal(readCallCount, 1);
    assert.equal(cachedReadCallCount, 0);

    await adapter.writeFile('Aether/Compiled/Approved.md', '# Approved\nEntry 1\n');
    assert.deepEqual(createdFolders, ['Aether', 'Aether/Compiled']);
    assert.equal(files.get('Aether/Compiled/Approved.md'), '# Approved\nEntry 1\n');

    await adapter.writeFile('Aether/Compiled/Approved.md', '# Approved\nEntry 2\n');
    assert.equal(files.get('Aether/Compiled/Approved.md'), '# Approved\nEntry 2\n');
    assert.equal(createdFolders.length, 2);

    const dispatched: VaultEvent[] = [];
    adapter.onVaultEvent(async (ev) => {
      dispatched.push(ev);
    });
    assert.equal(registeredEvents.length, 0);

    emitObsidian('create', makeFileObj('Startup/Old.md'));
    assert.equal(dispatched.length, 0);

    assert.ok(layoutReadyCallback);
    (layoutReadyCallback as () => void)();
    assert.equal(registeredEvents.length, 4);

    emitObsidian('modify', makeFileObj('Arch/DB.md'));
    emitObsidian('modify', makeFileObj('Arch/DB.md'));
    await new Promise((resolve) => setTimeout(resolve, 60));
    assert.equal(dispatched.length, 1);
    assert.deepEqual(dispatched[0], {kind: 'modify', path: 'Arch/DB.md'});

    emitObsidian('modify', makeFileObj('Arch/Old.md'));
    emitObsidian('rename', makeFileObj('Arch/New.md'), 'Arch/Old.md');
    emitObsidian('delete', makeFileObj('Arch/New.md'));
    await new Promise((resolve) => setTimeout(resolve, 60));
    assert.deepEqual(dispatched.slice(1), [
      {kind: 'rename', oldPath: 'Arch/Old.md', newPath: 'Arch/New.md'},
      {kind: 'delete', path: 'Arch/New.md'},
    ]);

    emitObsidian('modify', makeFileObj('Arch/PendingDispose.md'));
    adapter.dispose();
    await new Promise((resolve) => setTimeout(resolve, 50));
    assert.equal(dispatched.length, 3);

    const debouncer = new PathDebouncer(50);
    let flushedCount = 0;
    debouncer.enqueue({kind: 'modify', path: 'x.md'}, async () => {
      flushedCount += 1;
    });
    await debouncer.flushOrCancel('cancel');
    await new Promise((resolve) => setTimeout(resolve, 70));
    assert.equal(flushedCount, 0);
  });

  it('bridges AetherControlClient through Obsidian requestUrl transport and supports hot reconfigure', async () => {
    const requests: Array<{url: string; method?: string; auth?: string; body?: string}> = [];
    const mockRequestUrl = async (req: {
      url: string;
      method?: string;
      headers?: Record<string, string>;
      body?: string | ArrayBuffer;
    }) => {
      requests.push({
        url: req.url,
        method: req.method,
        auth: req.headers?.Authorization,
        body: typeof req.body === 'string' ? req.body : undefined,
      });
      const text = JSON.stringify({
        status: 'ok',
        schema_version: 3,
        embedder_id: 'hashed-ngram-256-v1',
        embedding_dim: 256,
        active_amus: 9,
        pending_inbox_count: 0,
        approved_inbox_count: 1,
        hot_tokens_used: 42,
        hot_token_budget: 2000,
        prefix_hash: '0'.repeat(64),
      });
      return {
        status: 200,
        headers: {'content-type': 'application/json'},
        arrayBuffer: new TextEncoder().encode(text).buffer,
        text,
        json: {},
      };
    };

    const transport = createRequestUrlFetch(mockRequestUrl);
    const mockHost = createMockHost();
    const plugin = new AetherCompanionPlugin(mockHost.host, 'http://127.0.0.1:14771', {
      fetchImpl: transport,
      apiKey: 'aek_initial_token',
      defaultKvModel: 'llama-3.1-8b',
    });
    await plugin.onload();
    assert.equal(requests[0]?.url, 'http://127.0.0.1:14771/v1/status');
    assert.equal(requests[0]?.auth, 'Bearer aek_initial_token');
    assert.equal(mockHost.statusTexts.at(-1), 'Aether: 9 AMUs | 0 Inbox | HOT 42/2000');

    plugin.reconfigure('https://vault.internal:14771', {
      fetchImpl: transport,
      apiKey: 'aek_rotated_token',
      defaultKvModel: 'qwen2.5-coder',
      runDreamOnPush: true,
    });
    await plugin.refreshStatus();
    assert.equal(requests[1]?.url, 'https://vault.internal:14771/v1/status');
    assert.equal(requests[1]?.auth, 'Bearer aek_rotated_token');
    assert.equal(plugin.defaultKvModel, 'qwen2.5-coder');
    assert.equal(plugin.runDreamOnPush, true);
  });
});

function createValidReleaseFixture() {
  const manifest = {
    id: 'aether-memory',
    name: 'Aether Memory Companion',
    version: '0.4.0',
    minAppVersion: '1.5.0',
    description: 'Privacy-preserving two-way vault sync, Git lattice sync, and KV-cache warming for Aether MCP.',
    author: 'Nytheris',
    authorUrl: 'https://github.com/Nytheris-AI/aether-sdk',
    isDesktopOnly: true,
  };
  const pkg = {
    name: 'nytheris-aether-obsidian-plugin',
    version: '0.4.0',
    private: true,
    type: 'module',
    devDependencies: {
      '@types/node': '^24.0.0',
      esbuild: '^0.25.0',
      obsidian: '^1.5.0',
      typescript: '~6.0.2',
    },
  };
  return {
    'main.js': '"use strict";module.exports=function(){};',
    'manifest.json': JSON.stringify(manifest, null, 2),
    'versions.json': JSON.stringify({'0.4.0': '1.5.0'}),
    'styles.css': '.aether-inbox-card { color: var(--text-normal); }',
    'README.md': '# Aether Memory Companion\n',
    LICENSE: 'Apache License Version 2.0\n',
    'package.json': JSON.stringify(pkg, null, 2),
    'tsconfig.json': JSON.stringify({compilerOptions: {typeRoots: ['./node_modules/@types']}}),
    'esbuild.config.mjs': "import esbuild from 'esbuild';\nexport default esbuild;\n",
  };
}

describe('Aether Obsidian Release Verification', () => {
  it('exposes the six required Obsidian Community Plugin release artifacts', () => {
    assert.deepEqual(
      [...REQUIRED_RELEASE_FILES].sort(),
      ['LICENSE', 'README.md', 'main.js', 'manifest.json', 'styles.css', 'versions.json'].sort(),
    );
  });

  it('passes validateRelease for valid artifacts, standalone config, and matching tag', () => {
    const files = createValidReleaseFixture();
    const withTag = validateRelease({files, tag: '0.4.0'});
    assert.equal(withTag.ok, true, formatVerificationReport(withTag));
    assert.equal(
      withTag.checks.every((check) => check.status === 'pass'),
      true,
      'expected every check to pass when --tag matches manifest.version',
    );

    const withoutTag = validateRelease({files});
    assert.equal(withoutTag.ok, true, formatVerificationReport(withoutTag));
    const tagCheck = withoutTag.checks.find((check) => check.id === 'release-tag-match');
    assert.equal(tagCheck?.status, 'skip');
  });

  it('rejects manifest ids containing obsidian, ending with plugin, or using invalid characters', () => {
    for (const badId of ['obsidian-aether', 'aether-plugin', 'aetherplugin', 'Aether_1', '1aether']) {
      const files = createValidReleaseFixture();
      const manifest = JSON.parse(files['manifest.json']);
      manifest.id = badId;
      files['manifest.json'] = JSON.stringify(manifest);

      const report = validateRelease({files});
      assert.equal(report.ok, false, `expected id "${badId}" to fail validation`);
      const idCheck = report.checks.find((check) => check.id === 'manifest-id-policy');
      assert.equal(idCheck?.status, 'fail', `expected manifest-id-policy failure for "${badId}"`);
    }
  });

  it('rejects missing or mismatched versions.json entries and package.json version mismatches', () => {
    const missingEntry = createValidReleaseFixture();
    missingEntry['versions.json'] = JSON.stringify({'0.3.0': '1.5.0'});
    const missingReport = validateRelease({files: missingEntry});
    assert.equal(missingReport.ok, false);
    assert.equal(
      missingReport.checks.find((c) => c.id === 'versions-json-parity')?.status,
      'fail',
    );

    const wrongMinApp = createValidReleaseFixture();
    wrongMinApp['versions.json'] = JSON.stringify({'0.4.0': '1.4.0'});
    const mismatchReport = validateRelease({files: wrongMinApp});
    assert.equal(mismatchReport.ok, false);
    assert.equal(
      mismatchReport.checks.find((c) => c.id === 'versions-json-parity')?.status,
      'fail',
    );

    const pkgMismatch = createValidReleaseFixture();
    const pkg = JSON.parse(pkgMismatch['package.json']);
    pkg.version = '0.3.9';
    pkgMismatch['package.json'] = JSON.stringify(pkg);
    const pkgReport = validateRelease({files: pkgMismatch});
    assert.equal(pkgReport.ok, false);
    assert.equal(
      pkgReport.checks.find((c) => c.id === 'package-json-standalone')?.status,
      'fail',
    );
  });

  it('rejects tags with v prefix or version mismatch and accepts exact SemVer tag', () => {
    const files = createValidReleaseFixture();

    const prefixed = validateRelease({files, tag: 'v0.4.0'});
    assert.equal(prefixed.ok, false);
    const prefixedCheck = prefixed.checks.find((c) => c.id === 'release-tag-match');
    assert.equal(prefixedCheck?.status, 'fail');
    assert.match(prefixedCheck?.message ?? '', /v/i);

    const mismatched = validateRelease({files, tag: '0.5.0'});
    assert.equal(mismatched.ok, false);
    assert.equal(mismatched.checks.find((c) => c.id === 'release-tag-match')?.status, 'fail');

    const accepted = validateRelease({files, tag: '0.4.0'});
    assert.equal(accepted.ok, true);
    assert.equal(accepted.checks.find((c) => c.id === 'release-tag-match')?.status, 'pass');
  });

  it('rejects missing or empty required release artifacts', () => {
    for (const fileName of REQUIRED_RELEASE_FILES) {
      const missingFiles: Record<string, string | null> = createValidReleaseFixture();
      missingFiles[fileName] = null;
      const missingReport = validateRelease({files: missingFiles});
      assert.equal(missingReport.ok, false, `expected missing ${fileName} to fail`);
      const missingArtifactCheck = missingReport.checks.find(
        (c) => c.id === 'required-release-artifacts',
      );
      assert.equal(missingArtifactCheck?.status, 'fail');
      assert.ok(missingArtifactCheck?.message.includes(fileName));

      const emptyFiles: Record<string, string | null> = createValidReleaseFixture();
      emptyFiles[fileName] = '   \n';
      const emptyReport = validateRelease({files: emptyFiles});
      assert.equal(emptyReport.ok, false, `expected empty ${fileName} to fail`);
      const emptyArtifactCheck = emptyReport.checks.find(
        (c) => c.id === 'required-release-artifacts',
      );
      assert.equal(emptyArtifactCheck?.status, 'fail');
      assert.ok(emptyArtifactCheck?.message.includes(fileName));
    }
  });

  it('rejects ../website and _npx references in standalone config files', () => {
    const badPkg = createValidReleaseFixture();
    badPkg['package.json'] = JSON.stringify({
      ...JSON.parse(badPkg['package.json']),
      scripts: {typecheck: '../website/node_modules/.bin/tsc -p tsconfig.json --noEmit'},
    });
    assert.equal(validateRelease({files: badPkg}).ok, false);

    const badTsconfig = createValidReleaseFixture();
    badTsconfig['tsconfig.json'] = JSON.stringify({
      compilerOptions: {typeRoots: ['../website/node_modules/@types']},
    });
    assert.equal(validateRelease({files: badTsconfig}).ok, false);

    const badEsbuildWebsite = createValidReleaseFixture();
    badEsbuildWebsite['esbuild.config.mjs'] =
      "const p = '../website/node_modules/typescript';\nexport default p;\n";
    assert.equal(validateRelease({files: badEsbuildWebsite}).ok, false);

    const badEsbuildNpx = createValidReleaseFixture();
    badEsbuildNpx['esbuild.config.mjs'] = "const p = '.npm/_npx';\nexport default p;\n";
    assert.equal(validateRelease({files: badEsbuildNpx}).ok, false);
  });

  it('parses valid CLI arguments and rejects invalid flags', () => {
    assert.deepEqual(parseCliArgs([], '/default/plugin'), {
      ok: true,
      dir: '/default/plugin',
    });
    assert.deepEqual(parseCliArgs(['--tag', '0.4.0'], '/default/plugin'), {
      ok: true,
      dir: '/default/plugin',
      tag: '0.4.0',
    });
    assert.deepEqual(parseCliArgs(['--tag=0.4.0', '--dir=/tmp/out'], '/default/plugin'), {
      ok: true,
      dir: path.resolve('/tmp/out'),
      tag: '0.4.0',
    });
    assert.equal(parseCliArgs(['--tag'], '/default/plugin').ok, false);
    assert.equal(parseCliArgs(['--tag='], '/default/plugin').ok, false);
    assert.equal(parseCliArgs(['--dir'], '/default/plugin').ok, false);
    assert.equal(parseCliArgs(['--unknown'], '/default/plugin').ok, false);
    assert.equal(parseCliArgs(['positional'], '/default/plugin').ok, false);
  });

  it('verifies a temp plugin directory through runReleaseCli and rejects broken directories', () => {
    const tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), 'aether-plugin-verify-'));
    try {
      const fixture = createValidReleaseFixture();
      for (const [relPath, content] of Object.entries(fixture)) {
        fs.writeFileSync(path.join(tmpDir, relPath), content, 'utf8');
      }
      const stdoutLines: string[] = [];
      const stderrLines: string[] = [];
      const okCode = runReleaseCli(['--dir', tmpDir, '--tag', '0.4.0'], tmpDir, {
        stdout: (line) => stdoutLines.push(line),
        stderr: (line) => stderrLines.push(line),
      });
      assert.equal(okCode, 0, stderrLines.join('\n'));
      assert.equal(stderrLines.length, 0);
      assert.ok(stdoutLines.join('\n').includes('PASS'));

      fs.rmSync(path.join(tmpDir, 'styles.css'));
      const failOut: string[] = [];
      const failErr: string[] = [];
      const failCode = runReleaseCli(['--dir', tmpDir], tmpDir, {
        stdout: (line) => failOut.push(line),
        stderr: (line) => failErr.push(line),
      });
      assert.equal(failCode, 1);
      assert.ok(failErr.join('\n').includes('styles.css'));
    } finally {
      fs.rmSync(tmpDir, {recursive: true, force: true});
    }
  });

  it('verifies tracked plugin files before build and full release directory when main.js is built', () => {
    const pluginRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
    const hasBundle = fs.existsSync(path.join(pluginRoot, 'main.js'));
    const report = verifyReleaseDirectory(pluginRoot, {tag: '0.4.1'});

    if (hasBundle) {
      assert.equal(report.ok, true, formatVerificationReport(report));
      assert.equal(
        report.checks.every((check) => check.status === 'pass'),
        true,
        formatVerificationReport(report),
      );
      return;
    }

    const artifactCheck = report.checks.find((c) => c.id === 'required-release-artifacts');
    assert.equal(artifactCheck?.status, 'fail');
    assert.equal(
      artifactCheck?.message,
      'Missing or empty release artifacts: main.js (missing)',
    );
    assert.equal(report.checks.find((c) => c.id === 'main-bundle-sanity')?.status, 'skip');
    for (const check of report.checks) {
      if (check.id !== 'required-release-artifacts' && check.id !== 'main-bundle-sanity') {
        assert.equal(check.status, 'pass', `${check.id}: ${check.message}`);
      }
    }
  });
});

describe('onboarding command builders', () => {
  it('formats local start and service install commands from vault path and endpoint URL', () => {
    assert.equal(
      buildLocalStartCommand('/Users/alice/My Vault', 'http://127.0.0.1:14771'),
      'aether-mcp --vault "/Users/alice/My Vault" --http-addr 127.0.0.1:14771 --http-only',
    );
    assert.equal(
      buildServiceInstallCommand('/Users/alice/My Vault', 'http://localhost:15000'),
      'aether-mcp service install --vault "/Users/alice/My Vault" --http-addr 127.0.0.1:15000',
    );
    assert.equal(
      buildLocalStartCommand('', 'https://aether.team.example.com'),
      'aether-mcp --vault "/path/to/your/vault" --http-addr 127.0.0.1:14771 --http-only',
    );
  });
});

describe('community plugin manifest', () => {
  it('leads with the agent memory bridge and follows Obsidian description rules', () => {
    const pluginRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
    const manifest = JSON.parse(fs.readFileSync(path.join(pluginRoot, 'manifest.json'), 'utf8'));
    assert.ok(manifest.description.length <= 250, 'description must be at most 250 characters');
    assert.ok(manifest.description.endsWith('.'), 'description must end with a period');
    assert.match(manifest.description, /^Give your AI agents memory/);
    assert.doesNotMatch(manifest.description, /obsidian/i);
  });
});
