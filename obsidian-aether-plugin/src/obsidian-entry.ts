import {ItemView, Notice, Plugin, type WorkspaceLeaf, requestUrl} from 'obsidian';
import type {AetherClientOptions} from './api.ts';
import {ObsidianDesktopAdapter, createRequestUrlFetch} from './desktop-adapter.ts';
import {
  AetherCompanionPlugin,
  type InboxReviewController,
  type ObsidianCommandDescriptor,
  renderInboxPane,
} from './main.ts';
import {
  type AetherPluginSettings,
  type AetherSettingsPluginHost,
  AetherSettingTab,
  DEFAULT_SETTINGS,
  normalizePluginSettings,
} from './settings.ts';
import {describeConnectionError} from './settings-model.ts';
import type {VaultPullResponse} from './types.ts';

const VIEW_TYPE_AETHER_INBOX = 'aether-inbox-review';

class InboxReviewView extends ItemView {
  private readonly controller: InboxReviewController;

  constructor(leaf: WorkspaceLeaf, controller: InboxReviewController) {
    super(leaf);
    this.controller = controller;
  }

  getViewType(): string {
    return VIEW_TYPE_AETHER_INBOX;
  }

  getDisplayText(): string {
    return 'Aether memory inbox';
  }

  async onOpen(): Promise<void> {
    this.render();
  }

  render(): void {
    renderInboxPane(this.contentEl, this.controller);
  }

  async onClose(): Promise<void> {
    this.contentEl.replaceChildren();
  }
}

function formatClockHm(date: Date): string {
  const hh = String(date.getHours()).padStart(2, '0');
  const mm = String(date.getMinutes()).padStart(2, '0');
  return `${hh}:${mm}`;
}

function statusTooltip(text: string): string {
  if (text === 'Aether: Offline') {
    return 'aether-mcp is not reachable. Click to try a sync again.';
  }
  if (/to review$/.test(text)) {
    return 'AI agents proposed memories. Click to review them.';
  }
  return 'Click to sync your vault with Aether.';
}

export default class ObsidianDesktopPlugin extends Plugin implements AetherSettingsPluginHost {
  settings: AetherPluginSettings = {...DEFAULT_SETTINGS};

  private formatError(prefix: string, err: unknown): string {
    return `${prefix}: ${describeConnectionError(err, this.settings.endpointUrl)}`;
  }

  private adapter: ObsidianDesktopAdapter | null = null;
  private companion: AetherCompanionPlugin | null = null;
  private statusBarEl: HTMLElement | null = null;
  private statusTextOverride: string | null = null;
  private reviewCount = 0;
  private autoPullIntervalId: number | null = null;

  async onload(): Promise<void> {
    this.settings = normalizePluginSettings(await this.loadData());
    this.statusBarEl = this.addStatusBarItem();
    this.statusBarEl.addClass('mod-clickable');
    this.setStatusText('Aether: Idle');
    this.statusBarEl.addEventListener('click', () => {
      if (this.reviewCount > 0) {
        void this.openInbox();
      } else {
        void this.runSyncWithStatus();
      }
    });

    const fetchImpl = createRequestUrlFetch(requestUrl);
    this.adapter = new ObsidianDesktopAdapter({
      app: this.app,
      registerEvent: (ref) => this.registerEvent(ref),
      addStatusBarItem: () => ({
        setText: (text: string) => {
          this.handleCompanionStatusText(text);
        },
      }),
      addCommand: (cmd) => {
        this.registerGuardedCommand(cmd);
      },
      notify: (message: string) => {
        new Notice(message);
      },
      openInboxPane: (controller) => {
        void this.activateInboxView(controller);
      },
      debounceMs: this.settings.syncDebounceMs,
      isLiveSyncEnabled: () => this.settings.liveSyncEnabled,
      onSyncError: (err) => {
        this.setStatusText('Aether: Offline');
        new Notice(this.formatError('Aether live sync failed', err));
      },
    });

    this.companion = new AetherCompanionPlugin(
      this.adapter,
      this.settings.endpointUrl,
      this.buildClientOptions(fetchImpl),
    );

    this.registerView(VIEW_TYPE_AETHER_INBOX, (leaf) => {
      return new InboxReviewView(leaf, this.companion!.inboxController);
    });

    this.addSettingTab(new AetherSettingTab(this.app, this));

    try {
      await this.companion.onload();
      if (this.statusTextOverride === null) {
        this.setStatusText('Aether: Idle');
      }
    } catch (err) {
      this.setStatusText('Aether: Offline');
      new Notice(this.formatError('Aether initialization warning', err));
    }

    this.configureAutoPullTimer();
  }

  onunload(): void {
    this.clearAutoPullTimer();
    this.adapter?.dispose();
    this.adapter = null;
    this.companion = null;
    this.statusBarEl = null;
  }

  async saveSettings(): Promise<void> {
    await this.saveData(this.settings);
  }

  reconfigureClient(): void {
    this.adapter?.setDebounceMs(this.settings.syncDebounceMs);
    this.configureAutoPullTimer();
    if (!this.companion) {
      return;
    }
    try {
      const fetchImpl = createRequestUrlFetch(requestUrl);
      this.companion.reconfigure(this.settings.endpointUrl, this.buildClientOptions(fetchImpl));
      this.statusTextOverride = null;
      void this.companion.refreshStatus();
    } catch (err) {
      this.setStatusText('Aether: Offline');
      new Notice(this.formatError('Aether settings error', err));
    }
  }

  async testConnection(): Promise<VaultPullResponse> {
    if (!this.companion) {
      throw new Error('Aether plugin is not initialized');
    }
    return this.companion.client.pullVaultState();
  }

  private buildClientOptions(fetchImpl: AetherClientOptions['fetchImpl']): AetherClientOptions {
    return {
      fetchImpl,
      apiKey: this.settings.apiKey || undefined,
      defaultKvModel: this.settings.kvModel || undefined,
      runDreamOnPush: this.settings.runDreamOnPush,
      kvWarmingEnabled: this.settings.kvWarmingEnabled,
    };
  }

  private registerGuardedCommand(cmd: ObsidianCommandDescriptor): void {
    const run = async (): Promise<void> => {
      if (cmd.id === 'aether-sync-vault') {
        await this.runSyncWithStatus();
        return;
      }
      try {
        await cmd.callback();
      } catch (err) {
        this.setStatusText('Aether: Offline');
        new Notice(this.formatError(`Aether ${cmd.name} failed`, err));
      }
    };
    this.addCommand({
      id: cmd.id,
      name: cmd.name,
      checkCallback: (checking: boolean) => {
        if (cmd.available && !cmd.available()) {
          return false;
        }
        if (!checking) {
          void run();
        }
        return true;
      },
    });
  }

  private async runSyncWithStatus(): Promise<void> {
    if (!this.companion) {
      return;
    }
    this.setStatusText('Aether: Syncing…');
    try {
      await this.companion.syncVaultWithAether({
        runDream: this.settings.runDreamOnPush,
      });
      this.setStatusText(`Aether: Synced (${formatClockHm(new Date())})`);
    } catch (err) {
      this.setStatusText('Aether: Offline');
      new Notice(this.formatError('Aether sync failed', err));
    }
  }

  private handleCompanionStatusText(text: string): void {
    const review = /^Aether: (\d+) to review$/.exec(text);
    this.reviewCount = review ? Number(review[1]) : 0;
    if (text === 'Aether: Offline') {
      this.setStatusText('Aether: Offline');
      return;
    }
    if (review || this.statusTextOverride === null || this.statusTextOverride === 'Aether: Offline') {
      this.setStatusText(review ? text : 'Aether: Idle');
    } else {
      this.setStatusText(this.statusTextOverride);
    }
  }

  private setStatusText(text: string): void {
    this.statusTextOverride = text;
    if (!this.statusBarEl) {
      return;
    }
    // Waiting reviews matter more than the last sync time, unless a sync or an outage is being reported.
    const showReview = this.reviewCount > 0 && /^Aether: (Idle|Synced)/.test(text);
    const shown = showReview ? `Aether: ${this.reviewCount} to review` : text;
    this.statusBarEl.textContent = shown;
    this.statusBarEl.setAttribute('aria-label', statusTooltip(shown));
  }

  private async openInbox(): Promise<void> {
    if (!this.companion) {
      return;
    }
    try {
      await this.companion.inboxController.refresh();
      await this.activateInboxView(this.companion.inboxController);
    } catch (err) {
      this.setStatusText('Aether: Offline');
      new Notice(this.formatError('Aether inbox failed to load', err));
    }
  }

  private configureAutoPullTimer(): void {
    this.clearAutoPullTimer();
    const intervalSec = this.settings.autoPullIntervalSec;
    if (intervalSec <= 0) {
      return;
    }
    const id = window.setInterval(() => {
      void this.companion?.pullMirrors().catch(() => {
        this.setStatusText('Aether: Offline');
      });
    }, intervalSec * 1000);
    this.autoPullIntervalId = this.registerInterval(id);
  }

  private clearAutoPullTimer(): void {
    if (this.autoPullIntervalId !== null) {
      window.clearInterval(this.autoPullIntervalId);
      this.autoPullIntervalId = null;
    }
  }

  private async activateInboxView(controller: InboxReviewController): Promise<void> {
    const existing = this.app.workspace.getLeavesOfType(VIEW_TYPE_AETHER_INBOX);
    const leaf = existing[0] ?? this.app.workspace.getRightLeaf(false);
    if (!leaf) {
      return;
    }
    await leaf.setViewState({type: VIEW_TYPE_AETHER_INBOX, active: true});
    await this.app.workspace.revealLeaf(leaf);
    if (leaf.view instanceof InboxReviewView) {
      renderInboxPane(leaf.view.contentEl, controller);
    }
  }
}
