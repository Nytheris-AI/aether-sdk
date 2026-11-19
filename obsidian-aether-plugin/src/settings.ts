import {type App, Notice, Plugin, PluginSettingTab, Setting} from 'obsidian';
import type {VaultPullResponse} from './types.ts';

export interface AetherPluginSettings {
  endpointUrl: string;
  apiKey: string;
  liveSyncEnabled: boolean;
  syncDebounceMs: number;
  autoPullIntervalSec: number;
  runDreamOnPush: boolean;
  kvEndpoint: string;
  kvModel: string;
}

export const DEFAULT_SETTINGS: AetherPluginSettings = {
  endpointUrl: 'http://127.0.0.1:3000',
  apiKey: '',
  liveSyncEnabled: true,
  syncDebounceMs: 1500,
  autoPullIntervalSec: 0,
  runDreamOnPush: false,
  kvEndpoint: '',
  kvModel: '',
};

export interface AetherSettingsPluginHost extends Plugin {
  settings: AetherPluginSettings;
  saveSettings(): Promise<void>;
  reconfigureClient(): void;
  testConnection(): Promise<VaultPullResponse>;
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function clampDebounceMs(raw: unknown): number {
  if (typeof raw !== 'number' || !Number.isFinite(raw)) {
    return DEFAULT_SETTINGS.syncDebounceMs;
  }
  return Math.min(10_000, Math.max(500, Math.round(raw)));
}

function normalizeNonNegativeInt(raw: unknown, fallback: number): number {
  if (typeof raw !== 'number' || !Number.isFinite(raw)) {
    return fallback;
  }
  return Math.max(0, Math.round(raw));
}

export function normalizePluginSettings(raw: unknown): AetherPluginSettings {
  if (!isPlainObject(raw)) {
    return {...DEFAULT_SETTINGS};
  }
  return {
    endpointUrl:
      typeof raw.endpointUrl === 'string' && raw.endpointUrl.trim().length > 0
        ? raw.endpointUrl.trim()
        : DEFAULT_SETTINGS.endpointUrl,
    apiKey: typeof raw.apiKey === 'string' ? raw.apiKey.trim() : DEFAULT_SETTINGS.apiKey,
    liveSyncEnabled:
      typeof raw.liveSyncEnabled === 'boolean'
        ? raw.liveSyncEnabled
        : DEFAULT_SETTINGS.liveSyncEnabled,
    syncDebounceMs: clampDebounceMs(raw.syncDebounceMs),
    autoPullIntervalSec: normalizeNonNegativeInt(
      raw.autoPullIntervalSec,
      DEFAULT_SETTINGS.autoPullIntervalSec,
    ),
    runDreamOnPush:
      typeof raw.runDreamOnPush === 'boolean'
        ? raw.runDreamOnPush
        : DEFAULT_SETTINGS.runDreamOnPush,
    kvEndpoint:
      typeof raw.kvEndpoint === 'string' ? raw.kvEndpoint.trim() : DEFAULT_SETTINGS.kvEndpoint,
    kvModel: typeof raw.kvModel === 'string' ? raw.kvModel.trim() : DEFAULT_SETTINGS.kvModel,
  };
}

export class AetherSettingTab extends PluginSettingTab {
  private readonly plugin: AetherSettingsPluginHost;

  constructor(app: App, plugin: AetherSettingsPluginHost) {
    super(app, plugin);
    this.plugin = plugin;
  }

  display(): void {
    const {containerEl} = this;
    containerEl.replaceChildren();

    new Setting(containerEl).setName('Connection & Auth').setHeading();

    new Setting(containerEl)
      .setName('Aether control-plane URL')
      .setDesc('Loopback http:// or remote https:// endpoint for aether-mcp --http-addr.')
      .addText((text) =>
        text
          .setPlaceholder(DEFAULT_SETTINGS.endpointUrl)
          .setValue(this.plugin.settings.endpointUrl)
          .onChange(async (value) => {
            this.plugin.settings.endpointUrl = value.trim() || DEFAULT_SETTINGS.endpointUrl;
            await this.applyAndPersist();
          }),
      );

    new Setting(containerEl)
      .setName('API key')
      .setDesc('Bearer token (aek_...) stored locally and excluded from Git team sync.')
      .addText((text) => {
        text.inputEl.type = 'password';
        return text
          .setPlaceholder('aek_...')
          .setValue(this.plugin.settings.apiKey)
          .onChange(async (value) => {
            this.plugin.settings.apiKey = value.trim();
            await this.applyAndPersist();
          });
      });

    new Setting(containerEl)
      .setName('Test connection')
      .setDesc('Verify server reachability and token permission via GET /v1/vault/pull.')
      .addButton((btn) =>
        btn
          .setButtonText('Test Connection')
          .setCta()
          .onClick(async () => {
            btn.setDisabled(true);
            try {
              const pulled = await this.plugin.testConnection();
              const count = Object.keys(pulled.manifest).length;
              new Notice(`Aether connected: ${count} indexed note(s) visible.`);
            } catch (err) {
              const message = err instanceof Error ? err.message : String(err);
              new Notice(`Aether connection failed: ${message}`);
            } finally {
              btn.setDisabled(false);
            }
          }),
      );

    new Setting(containerEl).setName('Live Sync & Lattice').setHeading();

    new Setting(containerEl)
      .setName('Live vault sync')
      .setDesc('Automatically sync note edits, renames, and deletions after startup layout.')
      .addToggle((toggle) =>
        toggle.setValue(this.plugin.settings.liveSyncEnabled).onChange(async (value) => {
          this.plugin.settings.liveSyncEnabled = value;
          await this.applyAndPersist();
        }),
      );

    new Setting(containerEl)
      .setName('Sync debounce (ms)')
      .setDesc('Delay before pushing rapid per-file edits (500 to 10000 ms).')
      .addSlider((slider) =>
        slider
          .setLimits(500, 10_000, 100)
          .setValue(this.plugin.settings.syncDebounceMs)
          .setDynamicTooltip()
          .onChange(async (value) => {
            this.plugin.settings.syncDebounceMs = value;
            await this.applyAndPersist();
          }),
      );

    new Setting(containerEl)
      .setName('Auto-pull interval (seconds)')
      .setDesc('Optional background poll to refresh Aether/Inbox.md and Approved.md (0 disables).')
      .addText((text) =>
        text
          .setPlaceholder('0')
          .setValue(String(this.plugin.settings.autoPullIntervalSec))
          .onChange(async (value) => {
            const parsed = Number(value);
            this.plugin.settings.autoPullIntervalSec = normalizeNonNegativeInt(parsed, 0);
            await this.applyAndPersist();
          }),
      );

    new Setting(containerEl)
      .setName('Run Dream consolidation on push')
      .setDesc('Requires an Admin-scoped API key when authentication is enabled on aether-mcp.')
      .addToggle((toggle) =>
        toggle.setValue(this.plugin.settings.runDreamOnPush).onChange(async (value) => {
          this.plugin.settings.runDreamOnPush = value;
          await this.applyAndPersist();
        }),
      );

    new Setting(containerEl).setName('Inference & KV-Warming').setHeading();

    new Setting(containerEl)
      .setName('KV endpoint (optional)')
      .setDesc('Inference endpoint associated with KV-cache warming telemetry.')
      .addText((text) =>
        text
          .setPlaceholder('http://127.0.0.1:8000')
          .setValue(this.plugin.settings.kvEndpoint)
          .onChange(async (value) => {
            this.plugin.settings.kvEndpoint = value.trim();
            await this.applyAndPersist();
          }),
      );

    new Setting(containerEl)
      .setName('Default KV model')
      .setDesc('Target model_id passed by the Warm GPU KV Cache command.')
      .addText((text) =>
        text
          .setPlaceholder('llama-3.1-8b')
          .setValue(this.plugin.settings.kvModel)
          .onChange(async (value) => {
            this.plugin.settings.kvModel = value.trim();
            await this.applyAndPersist();
          }),
      );
  }

  private async applyAndPersist(): Promise<void> {
    await this.plugin.saveSettings();
    this.plugin.reconfigureClient();
  }
}
