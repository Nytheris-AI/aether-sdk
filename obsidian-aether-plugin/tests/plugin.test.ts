import assert from 'node:assert/strict';
import http from 'node:http';
import {describe, it} from 'node:test';
import {AetherControlClient, assertLoopbackBaseUrl} from '../src/api.ts';
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
});
