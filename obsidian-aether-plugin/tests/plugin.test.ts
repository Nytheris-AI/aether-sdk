import assert from 'node:assert/strict';
import http from 'node:http';
import {describe, it} from 'node:test';
import {AetherControlClient, assertControlPlaneBaseUrl, assertLoopbackBaseUrl} from '../src/api.ts';
import {
  AetherCompanionPlugin,
  formatStatusBarText,
  type ObsidianCommandDescriptor,
  type ObsidianHostAdapter,
} from '../src/main.ts';
import type {InboxEntry, VaultStatusReport} from '../src/types.ts';

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
  it('enforces loopback URLs and rejects remote hosts', () => {
    assert.equal(assertLoopbackBaseUrl('http://127.0.0.1:4771'), 'http://127.0.0.1:4771');
    assert.equal(assertLoopbackBaseUrl('http://localhost:4771'), 'http://localhost:4771');
    assert.throws(() => assertLoopbackBaseUrl('http://192.168.1.50:4771'));
    assert.throws(() => assertLoopbackBaseUrl('https://127.0.0.1:4771'));
    assert.throws(() => new AetherControlClient('http://example.com:4771'));
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
      assertControlPlaneBaseUrl('https://team-vault.internal:4771', {apiKey: 'aek_test1234'}),
      'https://team-vault.internal:4771',
    );
    assert.throws(() => assertControlPlaneBaseUrl('https://team-vault.internal:4771'));
    assert.throws(() =>
      assertControlPlaneBaseUrl('http://192.168.1.50:4771', {apiKey: 'aek_test1234'}),
    );
    assert.equal(
      assertControlPlaneBaseUrl('http://192.168.1.50:4771', {
        apiKey: 'aek_test1234',
        allowInsecureRemoteHttp: true,
      }),
      'http://192.168.1.50:4771',
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
});
