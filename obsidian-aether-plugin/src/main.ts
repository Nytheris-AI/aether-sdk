import {AetherControlClient, type AetherClientOptions, type FetchLike} from './api.ts';
import type {
  DreamReport,
  GitSyncOptionsRequest,
  GitSyncReport,
  InboxDecision,
  InboxEntry,
  KvWarmReport,
  VaultStatusReport,
} from './types.ts';

export interface StatusBarHandle {
  setText(text: string): void;
}

export interface ObsidianCommandDescriptor {
  id: string;
  name: string;
  callback: () => Promise<unknown>;
}

export interface ObsidianHostAdapter {
  addStatusBarItem(): StatusBarHandle;
  addCommand(cmd: ObsidianCommandDescriptor): void;
  notify(message: string): void;
  openInboxPane(controller: InboxReviewController): void;
}

export function formatStatusBarText(status: VaultStatusReport | null): string {
  if (!status) {
    return 'Aether: Offline';
  }
  return `Aether: ${status.active_amus} AMUs | ${status.pending_inbox_count} Inbox | HOT ${status.hot_tokens_used}/${status.hot_token_budget}`;
}

export function formatDreamNotice(report: DreamReport): string {
  return `Aether Dream pass: scanned ${report.daily_notes_scanned} note(s), staged ${report.candidates_staged} candidate(s), flagged ${report.contradictions_flagged} contradiction(s).`;
}

export function formatGitSyncNotice(report: GitSyncReport): string {
  const statusLabel = report.merged_cleanly ? 'clean' : `${report.conflicted_files.length} conflict(s)`;
  return `Aether Git sync (${report.branch}, ${statusLabel}): indexed ${report.sync_report.amus_indexed} AMU(s), resolved ${report.aether_conflicts_resolved} Aether conflict(s).`;
}

export function formatKvWarmNotice(report: KvWarmReport): string {
  return `Aether KV warm (${report.model_id}): warmed ${report.blocks_warmed} block(s) across ${report.tokens_warmed} token(s), ${report.ttft_reduction_pct.toFixed(1)}% TTFT reduction.`;
}

/**
 * State controller backing the 1-click Inbox review pane inside Obsidian.
 */
export class InboxReviewController {
  pending: InboxEntry[] = [];
  lastError: string | null = null;
  private readonly client: AetherControlClient;
  private readonly onRefreshStatus: () => Promise<void>;

  constructor(client: AetherControlClient, onRefreshStatus: () => Promise<void>) {
    this.client = client;
    this.onRefreshStatus = onRefreshStatus;
  }

  async refresh(): Promise<InboxEntry[]> {
    const resp = await this.client.getInbox();
    this.pending = resp.pending;
    this.lastError = null;
    return this.pending;
  }

  async decide(candidateId: string, decision: InboxDecision): Promise<void> {
    await this.client.decideCandidate(candidateId, decision);
    this.pending = this.pending.filter((entry) => entry.candidate_id !== candidateId);
    this.lastError = null;
    await this.onRefreshStatus();
  }
}

/**
 * Safe DOM renderer for the 1-click Obsidian Inbox review pane (uses `textContent` only).
 */
export function renderInboxPane(
  container: HTMLElement,
  controller: InboxReviewController,
  onUpdated?: () => void,
): void {
  container.replaceChildren();
  const heading = container.ownerDocument.createElement('h3');
  heading.textContent = `Aether Inbox (${controller.pending.length} pending)`;
  container.appendChild(heading);

  if (controller.pending.length === 0) {
    const empty = container.ownerDocument.createElement('p');
    empty.textContent = 'No pending memory candidates.';
    container.appendChild(empty);
    return;
  }

  for (const entry of controller.pending) {
    const card = container.ownerDocument.createElement('div');
    card.className = 'aether-inbox-card';

    const meta = container.ownerDocument.createElement('div');
    meta.textContent = `[${entry.tier.toUpperCase()} | ${entry.scope}] (${entry.candidate_id})`;
    card.appendChild(meta);

    const fact = container.ownerDocument.createElement('p');
    fact.textContent = entry.proposed_fact;
    card.appendChild(fact);

    const actions = container.ownerDocument.createElement('div');
    const approveBtn = container.ownerDocument.createElement('button');
    approveBtn.textContent = 'Approve';
    approveBtn.onclick = async () => {
      await controller.decide(entry.candidate_id, 'approve');
      renderInboxPane(container, controller, onUpdated);
      onUpdated?.();
    };
    const rejectBtn = container.ownerDocument.createElement('button');
    rejectBtn.textContent = 'Reject';
    rejectBtn.onclick = async () => {
      await controller.decide(entry.candidate_id, 'reject');
      renderInboxPane(container, controller, onUpdated);
      onUpdated?.();
    };
    actions.appendChild(approveBtn);
    actions.appendChild(rejectBtn);
    card.appendChild(actions);
    container.appendChild(card);
  }
}

/**
 * Companion plugin wiring the Obsidian status bar, 1-click Inbox review pane,
 * Dream pass, Git sync, and KV-cache warming commands.
 */
export class AetherCompanionPlugin {
  readonly client: AetherControlClient;
  readonly inboxController: InboxReviewController;
  readonly defaultKvModel: string;
  private readonly host: ObsidianHostAdapter;
  private statusBar: StatusBarHandle | null = null;

  constructor(
    host: ObsidianHostAdapter,
    baseUrl = 'http://127.0.0.1:4771',
    fetchOrOptions: FetchLike | AetherClientOptions = fetch,
  ) {
    this.host = host;
    this.client = new AetherControlClient(baseUrl, fetchOrOptions);
    this.defaultKvModel =
      typeof fetchOrOptions === 'object' && fetchOrOptions.defaultKvModel
        ? fetchOrOptions.defaultKvModel
        : 'llama-3.1-8b';
    this.inboxController = new InboxReviewController(this.client, async () => {
      await this.refreshStatus();
    });
  }

  async onload(): Promise<void> {
    this.statusBar = this.host.addStatusBarItem();
    this.host.addCommand({
      id: 'aether-open-inbox-review',
      name: 'Open Aether Inbox Review',
      callback: async () => {
        await this.inboxController.refresh();
        this.host.openInboxPane(this.inboxController);
      },
    });
    this.host.addCommand({
      id: 'aether-trigger-dream-pass',
      name: 'Trigger Dream Pass',
      callback: async () => this.triggerDreamPass(),
    });
    this.host.addCommand({
      id: 'aether-git-sync',
      name: 'Git Sync Team Vault',
      callback: async () => this.gitSync(),
    });
    this.host.addCommand({
      id: 'aether-warm-kv-cache',
      name: 'Warm GPU KV Cache',
      callback: async () => this.warmKvCache(),
    });
    await this.refreshStatus();
  }

  async refreshStatus(): Promise<VaultStatusReport | null> {
    try {
      const status = await this.client.getStatus();
      this.statusBar?.setText(formatStatusBarText(status));
      return status;
    } catch {
      this.statusBar?.setText(formatStatusBarText(null));
      return null;
    }
  }

  async triggerDreamPass(): Promise<DreamReport> {
    const report = await this.client.triggerDreamPass();
    await this.inboxController.refresh();
    await this.refreshStatus();
    this.host.notify(formatDreamNotice(report));
    return report;
  }

  async gitSync(options: GitSyncOptionsRequest = {}): Promise<GitSyncReport> {
    const report = await this.client.gitSync(options);
    await this.inboxController.refresh();
    await this.refreshStatus();
    this.host.notify(formatGitSyncNotice(report));
    return report;
  }

  async warmKvCache(modelId = this.defaultKvModel, scope?: string): Promise<KvWarmReport> {
    const report = await this.client.warmKvCache({
      model_id: modelId,
      scope,
    });
    this.host.notify(formatKvWarmNotice(report));
    return report;
  }
}
