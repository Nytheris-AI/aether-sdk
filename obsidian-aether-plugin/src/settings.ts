import {type App, Notice, Plugin, PluginSettingTab, Setting} from 'obsidian';
import {
  AETHER_MCP_INSTALL_COMMAND,
  AETHER_MCP_WINDOWS_INSTALL_COMMAND,
  type AetherPluginSettings,
  buildLocalStartCommand,
  buildServiceInstallCommand,
  DEFAULT_SETTINGS,
  normalizeNonNegativeInt,
  normalizePluginSettings,
} from './settings-model.ts';
import type {VaultPullResponse} from './types.ts';

export {type AetherPluginSettings, DEFAULT_SETTINGS, normalizePluginSettings};

export interface AetherSettingsPluginHost extends Plugin {
  settings: AetherPluginSettings;
  saveSettings(): Promise<void>;
  reconfigureClient(): void;
  testConnection(): Promise<VaultPullResponse>;
}

async function copyCommandToClipboard(command: string, label: string): Promise<void> {
  if (typeof navigator !== 'undefined' && navigator.clipboard?.writeText) {
    await navigator.clipboard.writeText(command);
    new Notice(`Copied ${label} to clipboard.`);
    return;
  }
  new Notice(command);
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

    containerEl.createEl('p', {
      cls: 'setting-item-description',
      text: 'This plugin needs the aether-mcp server running on your machine or on a team gateway. Use Local server quickstart below to install and start it, then run Test Connection.',
    });

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

    new Setting(containerEl)
      .setName('Local server quickstart')
      .setDesc(
        'Copy commands to install aether-mcp, register it as a background service, or run it in a terminal.',
      )
      .addButton((btn) =>
        btn.setButtonText('Copy Install').onClick(async () => {
          const isWindows =
            typeof navigator !== 'undefined' && navigator.platform.toLowerCase().includes('win');
          const cmd = isWindows ? AETHER_MCP_WINDOWS_INSTALL_COMMAND : AETHER_MCP_INSTALL_COMMAND;
          await copyCommandToClipboard(cmd, 'install command');
        }),
      )
      .addButton((btn) =>
        btn.setButtonText('Copy Service Install').onClick(async () => {
          const cmd = buildServiceInstallCommand(
            this.resolveVaultBasePath(),
            this.plugin.settings.endpointUrl,
          );
          await copyCommandToClipboard(cmd, 'service install command');
        }),
      )
      .addButton((btn) =>
        btn.setButtonText('Copy Start').onClick(async () => {
          const cmd = buildLocalStartCommand(
            this.resolveVaultBasePath(),
            this.plugin.settings.endpointUrl,
          );
          await copyCommandToClipboard(cmd, 'start command');
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

    new Setting(containerEl).setName('Advanced: GPU KV-cache warming (optional)').setHeading();

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

  private resolveVaultBasePath(): string {
    const adapter = this.app.vault.adapter as {basePath?: unknown};
    return typeof adapter.basePath === 'string' ? adapter.basePath : '';
  }

  private async applyAndPersist(): Promise<void> {
    await this.plugin.saveSettings();
    this.plugin.reconfigureClient();
  }
}
