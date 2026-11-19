import type {App, EventRef, RequestUrlParam, RequestUrlResponse, TAbstractFile, TFile} from 'obsidian';
import type {FetchLike} from './api.ts';
import type {
  InboxReviewController,
  ObsidianCommandDescriptor,
  ObsidianHostAdapter,
  StatusBarHandle,
} from './main.ts';
import type {VaultEvent} from './types.ts';

export type RequestUrlLike = (request: RequestUrlParam) => Promise<RequestUrlResponse>;

export interface ObsidianDesktopAdapterOptions {
  app: App;
  registerEvent: (ref: EventRef) => void;
  addStatusBarItem: () => StatusBarHandle;
  addCommand: (cmd: ObsidianCommandDescriptor) => void;
  notify: (message: string) => void;
  openInboxPane: (controller: InboxReviewController) => void;
  debounceMs?: number;
  isLiveSyncEnabled?: () => boolean;
  onSyncError?: (error: unknown) => void;
}

interface PendingUpsert {
  event: Extract<VaultEvent, {kind: 'create' | 'modify'}>;
  timer: ReturnType<typeof setTimeout>;
  handler: (event: VaultEvent) => Promise<void>;
}

/**
 * Normalizes vault paths to POSIX form without leading `./` or `/`.
 */
export function normalizeObsidianPath(raw: string): string {
  let normalized = raw.trim().replace(/\\/g, '/');
  while (normalized.startsWith('./')) {
    normalized = normalized.slice(2);
  }
  normalized = normalized.replace(/^\/+/, '');
  const parts: string[] = [];
  for (const segment of normalized.split('/')) {
    if (segment === '' || segment === '.') {
      continue;
    }
    parts.push(segment);
  }
  return parts.join('/');
}

function isVaultFile(file: TAbstractFile | null): file is TFile {
  return file !== null && !('children' in file);
}

function isSyncCandidatePath(rawPath: string): boolean {
  const norm = normalizeObsidianPath(rawPath);
  return norm.toLowerCase().endsWith('.md') || norm === '.aetherignore';
}

function normalizeHeaders(headersInit: RequestInit['headers']): Record<string, string> | undefined {
  if (!headersInit) {
    return undefined;
  }
  const out: Record<string, string> = {};
  if (headersInit instanceof Headers) {
    headersInit.forEach((value, key) => {
      out[key] = value;
    });
    return Object.keys(out).length > 0 ? out : undefined;
  }
  if (Array.isArray(headersInit)) {
    for (const [key, value] of headersInit) {
      out[key] = value;
    }
    return Object.keys(out).length > 0 ? out : undefined;
  }
  for (const [key, value] of Object.entries(headersInit)) {
    if (value !== undefined) {
      out[key] = value;
    }
  }
  return Object.keys(out).length > 0 ? out : undefined;
}

function normalizeRequestBody(body: RequestInit['body']): string | undefined {
  if (body === undefined || body === null) {
    return undefined;
  }
  if (typeof body === 'string') {
    return body;
  }
  if (body instanceof Uint8Array) {
    return new TextDecoder().decode(body);
  }
  return String(body);
}

/**
 * Bridges Obsidian's CORS-free `requestUrl` API into a standard `fetch`-compatible transport.
 */
export function createRequestUrlFetch(requestUrlFn: RequestUrlLike): FetchLike {
  return async (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
    const url = typeof input === 'string' ? input : input instanceof URL ? input.toString() : input.url;
    const response = await requestUrlFn({
      url,
      method: init?.method ?? 'GET',
      headers: normalizeHeaders(init?.headers),
      body: normalizeRequestBody(init?.body),
      throw: false,
    });
    const status = response.status >= 200 && response.status <= 599 ? response.status : 500;
    const body = status === 204 || status === 304 ? null : response.text;
    return new Response(body, {
      status,
      headers: response.headers,
    });
  };
}

/**
 * Per-path debouncer that coalesces rapid `create`/`modify` events while executing
 * `delete` and `rename` transitions immediately in causal order.
 */
export class PathDebouncer {
  private debounceMs: number;
  private readonly pending = new Map<string, PendingUpsert>();
  private workChain: Promise<void> = Promise.resolve();
  private readonly onError?: (error: unknown) => void;

  constructor(debounceMs = 1500, onError?: (error: unknown) => void) {
    this.debounceMs = Math.max(0, debounceMs);
    this.onError = onError;
  }

  setDebounceMs(ms: number): void {
    this.debounceMs = Math.max(0, ms);
  }

  enqueue(
    event: Extract<VaultEvent, {kind: 'create' | 'modify'}>,
    handler: (event: VaultEvent) => Promise<void>,
  ): void {
    this.cancelPath(event.path);
    const timer = setTimeout(() => {
      this.pending.delete(event.path);
      this.queueDispatch(event, handler);
    }, this.debounceMs);
    this.pending.set(event.path, {event, timer, handler});
  }

  dispatchImmediate(
    event: Extract<VaultEvent, {kind: 'delete' | 'rename'}>,
    handler: (event: VaultEvent) => Promise<void>,
  ): Promise<void> {
    if (event.kind === 'delete') {
      this.cancelPath(event.path);
    } else {
      this.cancelPath(event.oldPath);
      this.cancelPath(event.newPath);
    }
    return this.queueDispatch(event, handler);
  }

  async flushOrCancel(mode: 'cancel' | 'flush' = 'cancel'): Promise<void> {
    const entries = [...this.pending.values()];
    this.pending.clear();
    for (const entry of entries) {
      clearTimeout(entry.timer);
    }
    if (mode === 'flush') {
      for (const entry of entries) {
        this.queueDispatch(entry.event, entry.handler);
      }
    }
    await this.workChain;
  }

  private cancelPath(path: string): void {
    const existing = this.pending.get(path);
    if (!existing) {
      return;
    }
    clearTimeout(existing.timer);
    this.pending.delete(path);
  }

  private queueDispatch(
    event: VaultEvent,
    handler: (event: VaultEvent) => Promise<void>,
  ): Promise<void> {
    const next = this.workChain.then(async () => {
      try {
        await handler(event);
      } catch (err) {
        this.onError?.(err);
      }
    });
    this.workChain = next;
    return next;
  }
}

/**
 * Concrete Obsidian Desktop adapter implementing `ObsidianHostAdapter` without leaking
 * runtime `"obsidian"` imports into `src/main.ts` or headless tests.
 */
export class ObsidianDesktopAdapter implements ObsidianHostAdapter {
  private readonly app: App;
  private readonly registerEvent: (ref: EventRef) => void;
  private readonly statusBarFactory: () => StatusBarHandle;
  private readonly commandRegistrar: (cmd: ObsidianCommandDescriptor) => void;
  private readonly notifier: (message: string) => void;
  private readonly inboxPaneOpener: (controller: InboxReviewController) => void;
  private readonly isLiveSyncEnabled: () => boolean;
  private readonly debouncer: PathDebouncer;
  private disposed = false;

  constructor(options: ObsidianDesktopAdapterOptions) {
    this.app = options.app;
    this.registerEvent = options.registerEvent;
    this.statusBarFactory = options.addStatusBarItem;
    this.commandRegistrar = options.addCommand;
    this.notifier = options.notify;
    this.inboxPaneOpener = options.openInboxPane;
    this.isLiveSyncEnabled = options.isLiveSyncEnabled ?? (() => true);
    this.debouncer = new PathDebouncer(options.debounceMs ?? 1500, options.onSyncError);
  }

  addStatusBarItem(): StatusBarHandle {
    return this.statusBarFactory();
  }

  addCommand(cmd: ObsidianCommandDescriptor): void {
    this.commandRegistrar(cmd);
  }

  notify(message: string): void {
    this.notifier(message);
  }

  openInboxPane(controller: InboxReviewController): void {
    this.inboxPaneOpener(controller);
  }

  setDebounceMs(ms: number): void {
    this.debouncer.setDebounceMs(ms);
  }

  async listMarkdownFiles(): Promise<string[]> {
    return this.app.vault.getMarkdownFiles().map((file) => normalizeObsidianPath(file.path));
  }

  async readFile(path: string): Promise<string | null> {
    const norm = normalizeObsidianPath(path);
    const abstractFile = this.app.vault.getAbstractFileByPath(norm);
    if (!isVaultFile(abstractFile)) {
      return null;
    }
    return this.app.vault.read(abstractFile);
  }

  async writeFile(path: string, content: string): Promise<void> {
    const norm = normalizeObsidianPath(path);
    if (norm.length === 0) {
      return;
    }
    await this.ensureParentFolders(norm);
    const existing = this.app.vault.getAbstractFileByPath(norm);
    if (isVaultFile(existing)) {
      await this.app.vault.modify(existing, content);
      return;
    }
    if (existing === null) {
      await this.app.vault.create(norm, content);
    }
  }

  onVaultEvent(handler: (event: VaultEvent) => Promise<void>): void {
    this.app.workspace.onLayoutReady(() => {
      if (this.disposed) {
        return;
      }
      this.registerEvent(
        this.app.vault.on('create', (file) => {
          if (this.disposed || !this.isLiveSyncEnabled() || !isVaultFile(file)) {
            return;
          }
          const path = normalizeObsidianPath(file.path);
          if (!isSyncCandidatePath(path)) {
            return;
          }
          this.debouncer.enqueue({kind: 'create', path}, handler);
        }),
      );
      this.registerEvent(
        this.app.vault.on('modify', (file) => {
          if (this.disposed || !this.isLiveSyncEnabled() || !isVaultFile(file)) {
            return;
          }
          const path = normalizeObsidianPath(file.path);
          if (!isSyncCandidatePath(path)) {
            return;
          }
          this.debouncer.enqueue({kind: 'modify', path}, handler);
        }),
      );
      this.registerEvent(
        this.app.vault.on('delete', (file) => {
          if (this.disposed || !this.isLiveSyncEnabled() || !isVaultFile(file)) {
            return;
          }
          const path = normalizeObsidianPath(file.path);
          if (!isSyncCandidatePath(path)) {
            return;
          }
          void this.debouncer.dispatchImmediate({kind: 'delete', path}, handler);
        }),
      );
      this.registerEvent(
        this.app.vault.on('rename', (file, oldPath) => {
          if (this.disposed || !this.isLiveSyncEnabled() || !isVaultFile(file)) {
            return;
          }
          const normOld = normalizeObsidianPath(oldPath);
          const normNew = normalizeObsidianPath(file.path);
          if (!isSyncCandidatePath(normOld) && !isSyncCandidatePath(normNew)) {
            return;
          }
          void this.debouncer.dispatchImmediate(
            {kind: 'rename', oldPath: normOld, newPath: normNew},
            handler,
          );
        }),
      );
    });
  }

  dispose(): void {
    this.disposed = true;
    void this.debouncer.flushOrCancel('cancel');
  }

  private async ensureParentFolders(filePath: string): Promise<void> {
    const segments = filePath.split('/');
    segments.pop();
    let current = '';
    for (const seg of segments) {
      current = current.length === 0 ? seg : `${current}/${seg}`;
      if (this.app.vault.getAbstractFileByPath(current) !== null) {
        continue;
      }
      try {
        await this.app.vault.createFolder(current);
      } catch {
        if (this.app.vault.getAbstractFileByPath(current) === null) {
          throw new Error(`Failed to create vault folder "${current}"`);
        }
      }
    }
  }
}
