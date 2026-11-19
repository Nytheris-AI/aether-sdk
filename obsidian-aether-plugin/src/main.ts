import {
  AetherControlClient,
  DEFAULT_CONTROL_PLANE_URL,
  type AetherClientOptions,
  type FetchLike,
} from './api.ts';
import type {
  DreamReport,
  GitSyncOptionsRequest,
  GitSyncReport,
  InboxDecision,
  InboxEntry,
  KvWarmReport,
  SyncReport,
  VaultEvent,
  VaultNotePush,
  VaultPullResponse,
  VaultPushRequest,
  VaultPushResponse,
  VaultStatusReport,
} from './types.ts';

const INBOX_REL_PATH = 'Aether/Inbox.md';
const APPROVED_MIRROR_REL_PATH = 'Aether/Compiled/Approved.md';
const BUILTIN_IGNORED_DIRS = new Set(['.obsidian', '.git', '.trash', '.aether']);
const BUILTIN_IGNORED_PREFIXES = [
  'aether/inbox.md',
  'aether/compiled',
  '.obsidian/plugins/obsidian-aether/data.json',
];
const BUILTIN_IGNORED_EXTENSIONS = ['.canvas', '.excalidraw', '.excalidraw.md'];

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
  listMarkdownFiles?(): Promise<string[]>;
  readFile?(path: string): Promise<string | null>;
  writeFile?(path: string, content: string): Promise<void>;
  onVaultEvent?(handler: (event: VaultEvent) => Promise<void>): void;
}

export function normalizeVaultRelPath(raw: string): string {
  let normalized = raw.trim().replace(/\\/g, '/');
  while (normalized.startsWith('./')) {
    normalized = normalized.slice(2);
  }
  return normalized.replace(/^\/+/, '');
}

function unquoteYamlValue(raw: string): string {
  const trimmed = raw.trim();
  if (
    (trimmed.startsWith('"') && trimmed.endsWith('"') && trimmed.length >= 2) ||
    (trimmed.startsWith("'") && trimmed.endsWith("'") && trimmed.length >= 2)
  ) {
    return trimmed.slice(1, -1).trim();
  }
  return trimmed;
}

/**
 * Inspects YAML frontmatter and returns `true` when `aether_tier: private` is set.
 */
export function isPrivateMarkdownNote(content: string): boolean {
  let body: string | null = null;
  if (content.startsWith('---\n')) {
    body = content.slice(4);
  } else if (content.startsWith('---\r\n')) {
    body = content.slice(5);
  }
  if (body === null) {
    return false;
  }

  const lines = body.split(/\r?\n/);
  for (const line of lines) {
    const trimmed = line.trim();
    if (trimmed === '---') {
      return false;
    }
    if (trimmed.length === 0 || trimmed.startsWith('#')) {
      continue;
    }
    const colonIdx = trimmed.indexOf(':');
    if (colonIdx < 0) {
      continue;
    }
    const key = trimmed.slice(0, colonIdx).trim().toLowerCase();
    if (key === 'aether_tier') {
      const val = unquoteYamlValue(trimmed.slice(colonIdx + 1)).toLowerCase();
      return val === 'private';
    }
  }
  return false;
}

function pathMatchesPrefix(path: string, prefix: string): boolean {
  return path === prefix || path.startsWith(`${prefix}/`);
}

function globMatch(pattern: string, text: string): boolean {
  const escaped = pattern.replace(/[.+^${}()|[\]\\]/g, '\\$&');
  const regexSrc = `^${escaped.replace(/\*/g, '.*').replace(/\?/g, '.')}$`;
  return new RegExp(regexSrc).test(text);
}

function matchesIgnorePattern(rel: string, pattern: string): boolean {
  const segments = rel.split('/');
  if (pattern.endsWith('/')) {
    const dir = pattern.slice(0, -1);
    return pathMatchesPrefix(rel, dir) || segments.includes(dir);
  }
  if (pattern.includes('*') || pattern.includes('?')) {
    if (pattern.includes('/')) {
      return globMatch(pattern, rel);
    }
    const base = segments[segments.length - 1] ?? rel;
    return globMatch(pattern, base) || globMatch(pattern, rel);
  }
  if (pattern.includes('/')) {
    return pathMatchesPrefix(rel, pattern);
  }
  return rel === pattern || segments.includes(pattern);
}

/**
 * Evaluates built-in Aether vault exclusions and optional `.aetherignore` patterns.
 */
export function isIgnoredVaultPath(relPath: string, aetherignoreContent?: string): boolean {
  const rel = normalizeVaultRelPath(relPath);
  if (rel.length === 0) {
    return true;
  }
  const lower = rel.toLowerCase();
  if (BUILTIN_IGNORED_PREFIXES.some((prefix) => pathMatchesPrefix(lower, prefix))) {
    return true;
  }
  if (BUILTIN_IGNORED_EXTENSIONS.some((ext) => lower.endsWith(ext))) {
    return true;
  }
  if (!lower.endsWith('.md')) {
    return true;
  }
  if (lower.split('/').some((seg) => BUILTIN_IGNORED_DIRS.has(seg))) {
    return true;
  }
  if (!aetherignoreContent) {
    return false;
  }

  const patterns = aetherignoreContent
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter((line) => line.length > 0 && !line.startsWith('#'))
    .map(normalizeVaultRelPath)
    .filter((pat) => pat.length > 0);
  return patterns.some((pat) => matchesIgnorePattern(rel, pat));
}

export async function computeSha256Hex(content: string): Promise<string> {
  const bytes = new TextEncoder().encode(content);
  const digest = await globalThis.crypto.subtle.digest('SHA-256', bytes);
  return Array.from(new Uint8Array(digest), (b) => b.toString(16).padStart(2, '0')).join('');
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

export function formatVaultSyncNotice(report: SyncReport): string {
  return `Aether vault sync: updated ${report.files_updated} file(s), indexed ${report.amus_indexed} AMU(s), removed ${report.files_removed} file(s), skipped ${report.private_skipped} private.`;
}

/**
 * State controller backing the 1-click Inbox review pane inside Obsidian.
 */
export class InboxReviewController {
  pending: InboxEntry[] = [];
  lastError: string | null = null;
  private readonly getClient: () => AetherControlClient;
  private readonly onRefreshStatus: () => Promise<void>;

  constructor(
    clientOrGetter: AetherControlClient | (() => AetherControlClient),
    onRefreshStatus: () => Promise<void>,
  ) {
    this.getClient = typeof clientOrGetter === 'function' ? clientOrGetter : () => clientOrGetter;
    this.onRefreshStatus = onRefreshStatus;
  }

  async refresh(): Promise<InboxEntry[]> {
    const resp = await this.getClient().getInbox();
    this.pending = resp.pending;
    this.lastError = null;
    return this.pending;
  }

  async decide(candidateId: string, decision: InboxDecision): Promise<void> {
    await this.getClient().decideCandidate(candidateId, decision);
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
  client: AetherControlClient;
  readonly inboxController: InboxReviewController;
  defaultKvModel: string;
  runDreamOnPush: boolean;
  private readonly host: ObsidianHostAdapter;
  private readonly lastWrittenHashes = new Map<string, string>();
  private statusBar: StatusBarHandle | null = null;

  constructor(
    host: ObsidianHostAdapter,
    baseUrl = DEFAULT_CONTROL_PLANE_URL,
    fetchOrOptions: FetchLike | AetherClientOptions = fetch,
  ) {
    this.host = host;
    this.client = new AetherControlClient(baseUrl, fetchOrOptions);
    this.defaultKvModel = resolveDefaultKvModel(fetchOrOptions);
    this.runDreamOnPush = resolveRunDreamOnPush(fetchOrOptions);
    this.inboxController = new InboxReviewController(
      () => this.client,
      async () => {
        await this.syncMirrorsFromServer();
        await this.refreshStatus();
      },
    );
  }

  reconfigure(
    baseUrl: string,
    fetchOrOptions: FetchLike | AetherClientOptions = fetch,
  ): void {
    this.client = new AetherControlClient(baseUrl, fetchOrOptions);
    this.defaultKvModel = resolveDefaultKvModel(fetchOrOptions);
    this.runDreamOnPush = resolveRunDreamOnPush(fetchOrOptions);
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
      id: 'aether-sync-vault',
      name: 'Sync Vault with Aether',
      callback: async () => this.syncVaultWithAether(),
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
    this.host.onVaultEvent?.(async (event) => {
      await this.handleVaultEvent(event);
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

  async syncVaultWithAether(options: {runDream?: boolean} = {}): Promise<VaultPushResponse> {
    const pulled = await this.client.pullVaultState();
    const pushReq = await this.buildDeltaPushRequest(
      pulled,
      options.runDream ?? this.runDreamOnPush,
    );
    const resp = await this.pushAndApplyMirrors(pushReq);
    this.host.notify(formatVaultSyncNotice(resp.sync_report));
    return resp;
  }

  async pullMirrors(): Promise<void> {
    await this.syncMirrorsFromServer();
    await this.inboxController.refresh();
    await this.refreshStatus();
  }

  async handleVaultEvent(event: VaultEvent): Promise<VaultPushResponse | null> {
    switch (event.kind) {
      case 'create':
      case 'modify':
        return this.handleUpsertVaultEvent(normalizeVaultRelPath(event.path));
      case 'delete':
        return this.handleDeleteVaultEvent(normalizeVaultRelPath(event.path));
      case 'rename':
        return this.handleRenameVaultEvent(
          normalizeVaultRelPath(event.oldPath),
          normalizeVaultRelPath(event.newPath),
        );
      default: {
        const _exhaustive: never = event;
        return _exhaustive;
      }
    }
  }

  async triggerDreamPass(): Promise<DreamReport> {
    const report = await this.client.triggerDreamPass();
    await this.syncMirrorsFromServer();
    await this.inboxController.refresh();
    await this.refreshStatus();
    this.host.notify(formatDreamNotice(report));
    return report;
  }

  async gitSync(options: GitSyncOptionsRequest = {}): Promise<GitSyncReport> {
    const report = await this.client.gitSync(options);
    await this.syncMirrorsFromServer();
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

  private async handleUpsertVaultEvent(relPath: string): Promise<VaultPushResponse | null> {
    if (relPath === '.aetherignore') {
      return this.syncVaultWithAether();
    }
    if (relPath.toLowerCase() === APPROVED_MIRROR_REL_PATH.toLowerCase()) {
      return null;
    }
    if (!this.host.readFile) {
      return null;
    }

    const content = await this.host.readFile(relPath);
    if (content === null) {
      return null;
    }

    if (relPath.toLowerCase() === INBOX_REL_PATH.toLowerCase()) {
      const hash = await computeSha256Hex(content);
      if (this.lastWrittenHashes.get(normalizeVaultRelPath(INBOX_REL_PATH)) === hash) {
        return null;
      }
      return this.pushAndApplyMirrors({inbox_markdown: content});
    }

    const aetherignore = (await this.host.readFile('.aetherignore')) ?? undefined;
    if (isIgnoredVaultPath(relPath, aetherignore)) {
      return null;
    }

    if (isPrivateMarkdownNote(content)) {
      return this.pushAndApplyMirrors({
        private_paths: [relPath],
        deleted_paths: [relPath],
      });
    }
    return this.pushAndApplyMirrors({notes: [{path: relPath, content}]});
  }

  private async handleDeleteVaultEvent(relPath: string): Promise<VaultPushResponse | null> {
    if (relPath === '.aetherignore') {
      await this.pushAndApplyMirrors({aetherignore: ''});
      return this.syncVaultWithAether();
    }
    if (isManagedMirrorPath(relPath) || isIgnoredVaultPath(relPath)) {
      return null;
    }
    return this.pushAndApplyMirrors({deleted_paths: [relPath]});
  }

  private async handleRenameVaultEvent(
    oldPath: string,
    newPath: string,
  ): Promise<VaultPushResponse | null> {
    const aetherignore = (await this.host.readFile?.('.aetherignore')) ?? undefined;
    const deletedPaths: string[] = [];
    const notes: VaultNotePush[] = [];
    const privatePaths: string[] = [];

    if (!isManagedMirrorPath(oldPath) && !isIgnoredVaultPath(oldPath, aetherignore)) {
      deletedPaths.push(oldPath);
    }
    if (!isManagedMirrorPath(newPath) && !isIgnoredVaultPath(newPath, aetherignore)) {
      const newContent = await this.host.readFile?.(newPath);
      if (newContent !== null && newContent !== undefined) {
        if (isPrivateMarkdownNote(newContent)) {
          privatePaths.push(newPath);
        } else {
          notes.push({path: newPath, content: newContent});
        }
      }
    }

    if (deletedPaths.length === 0 && notes.length === 0 && privatePaths.length === 0) {
      return null;
    }
    return this.pushAndApplyMirrors({
      notes,
      deleted_paths: deletedPaths,
      private_paths: privatePaths,
    });
  }

  private async buildDeltaPushRequest(
    pulled: VaultPullResponse,
    runDream?: boolean,
  ): Promise<VaultPushRequest> {
    if (!this.host.listMarkdownFiles || !this.host.readFile) {
      return {run_dream: runDream};
    }

    const aetherignore = (await this.host.readFile('.aetherignore')) ?? undefined;
    const localFiles = await this.host.listMarkdownFiles();
    const seenPaths = new Set<string>();
    const notes: VaultNotePush[] = [];
    const privatePaths: string[] = [];
    const deletedPaths: string[] = [];

    let inboxMarkdown: string | undefined;
    const localInbox = await this.host.readFile(pulled.inbox_path);
    if (localInbox !== null) {
      const localInboxHash = await computeSha256Hex(localInbox);
      const pulledInboxHash = await computeSha256Hex(pulled.inbox_markdown);
      const selfHash = this.lastWrittenHashes.get(normalizeVaultRelPath(pulled.inbox_path));
      if (localInboxHash !== selfHash && localInboxHash !== pulledInboxHash) {
        inboxMarkdown = localInbox;
      }
    }

    for (const rawPath of localFiles) {
      const relPath = normalizeVaultRelPath(rawPath);
      if (
        relPath.toLowerCase() === pulled.inbox_path.toLowerCase() ||
        relPath.toLowerCase() === pulled.approved_mirror_path.toLowerCase() ||
        isIgnoredVaultPath(relPath, aetherignore)
      ) {
        continue;
      }
      const content = await this.host.readFile(relPath);
      if (content === null) {
        continue;
      }
      seenPaths.add(relPath);
      if (isPrivateMarkdownNote(content)) {
        privatePaths.push(relPath);
        continue;
      }
      const localHash = await computeSha256Hex(content);
      if (pulled.manifest[relPath] !== localHash) {
        notes.push({path: relPath, content});
      }
    }

    for (const remotePath of Object.keys(pulled.manifest)) {
      if (!seenPaths.has(remotePath)) {
        deletedPaths.push(remotePath);
      }
    }

    return {
      notes,
      deleted_paths: deletedPaths,
      private_paths: privatePaths,
      inbox_markdown: inboxMarkdown,
      aetherignore,
      run_dream: runDream,
    };
  }

  private async pushAndApplyMirrors(req: VaultPushRequest): Promise<VaultPushResponse> {
    const resp = await this.client.pushVaultChanges(req);
    await this.writeManagedMirrorFiles(
      resp.inbox_path,
      resp.inbox_markdown,
      resp.approved_mirror_path,
      resp.approved_mirror_markdown,
    );
    await this.inboxController.refresh();
    await this.refreshStatus();
    return resp;
  }

  private async syncMirrorsFromServer(): Promise<void> {
    if (!this.host.writeFile) {
      return;
    }
    const pulled = await this.client.pullVaultState();
    await this.writeManagedMirrorFiles(
      pulled.inbox_path,
      pulled.inbox_markdown,
      pulled.approved_mirror_path,
      pulled.approved_mirror_markdown,
    );
  }

  private async writeManagedMirrorFiles(
    inboxPath: string,
    inboxMarkdown: string,
    approvedPath: string,
    approvedMarkdown: string,
  ): Promise<void> {
    await this.writeManagedFile(inboxPath, inboxMarkdown);
    if (approvedMarkdown.length > 0) {
      await this.writeManagedFile(approvedPath, approvedMarkdown);
    }
  }

  private async writeManagedFile(path: string, content: string): Promise<void> {
    if (!this.host.writeFile) {
      return;
    }
    const normPath = normalizeVaultRelPath(path);
    const nextHash = await computeSha256Hex(content);
    const existing = this.host.readFile ? await this.host.readFile(path) : null;
    if (existing !== null && (await computeSha256Hex(existing)) === nextHash) {
      this.lastWrittenHashes.set(normPath, nextHash);
      return;
    }
    this.lastWrittenHashes.set(normPath, nextHash);
    await this.host.writeFile(path, content);
  }
}

function resolveDefaultKvModel(fetchOrOptions: FetchLike | AetherClientOptions): string {
  return typeof fetchOrOptions === 'object' && fetchOrOptions.defaultKvModel
    ? fetchOrOptions.defaultKvModel
    : 'llama-3.1-8b';
}

function resolveRunDreamOnPush(fetchOrOptions: FetchLike | AetherClientOptions): boolean {
  return typeof fetchOrOptions === 'object' && Boolean(fetchOrOptions.runDreamOnPush);
}

function isManagedMirrorPath(relPath: string): boolean {
  const lower = relPath.toLowerCase();
  return (
    lower === INBOX_REL_PATH.toLowerCase() ||
    lower === APPROVED_MIRROR_REL_PATH.toLowerCase()
  );
}
