import {DEFAULT_CONTROL_PLANE_URL} from './api.ts';

export interface AetherPluginSettings {
  endpointUrl: string;
  apiKey: string;
  liveSyncEnabled: boolean;
  syncDebounceMs: number;
  autoPullIntervalSec: number;
  runDreamOnPush: boolean;
  kvWarmingEnabled: boolean;
  kvEndpoint: string;
  kvModel: string;
}

export const DEFAULT_SETTINGS: AetherPluginSettings = {
  endpointUrl: DEFAULT_CONTROL_PLANE_URL,
  apiKey: '',
  liveSyncEnabled: true,
  syncDebounceMs: 1500,
  autoPullIntervalSec: 0,
  runDreamOnPush: false,
  kvWarmingEnabled: false,
  kvEndpoint: '',
  kvModel: '',
};

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function clampDebounceMs(raw: unknown): number {
  if (typeof raw !== 'number' || !Number.isFinite(raw)) {
    return DEFAULT_SETTINGS.syncDebounceMs;
  }
  return Math.min(10_000, Math.max(500, Math.round(raw)));
}

export function normalizeNonNegativeInt(raw: unknown, fallback: number): number {
  if (typeof raw !== 'number' || !Number.isFinite(raw)) {
    return fallback;
  }
  return Math.max(0, Math.round(raw));
}

export function normalizePluginSettings(raw: unknown): AetherPluginSettings {
  if (!isPlainObject(raw)) {
    return {...DEFAULT_SETTINGS};
  }
  const kvEndpoint =
    typeof raw.kvEndpoint === 'string' ? raw.kvEndpoint.trim() : DEFAULT_SETTINGS.kvEndpoint;
  const kvModel = typeof raw.kvModel === 'string' ? raw.kvModel.trim() : DEFAULT_SETTINGS.kvModel;
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
    // Existing KV configuration keeps the feature on after upgrading; new installs start with it off.
    kvWarmingEnabled:
      typeof raw.kvWarmingEnabled === 'boolean'
        ? raw.kvWarmingEnabled
        : kvEndpoint.length > 0 || kvModel.length > 0,
    kvEndpoint,
    kvModel,
  };
}

export const AETHER_MCP_INSTALL_COMMAND =
  'curl -fsSL https://raw.githubusercontent.com/muninn-hq/aether-sdk/main/scripts/install.sh | sh';

export const AETHER_MCP_WINDOWS_INSTALL_COMMAND =
  'irm https://raw.githubusercontent.com/muninn-hq/aether-sdk/main/scripts/install.ps1 | iex';

function resolveLocalHttpAddr(endpointUrl: string): string {
  try {
    const parsed = new URL(endpointUrl);
    if (
      parsed.protocol === 'http:' &&
      (parsed.hostname === '127.0.0.1' ||
        parsed.hostname === 'localhost' ||
        parsed.hostname === '::1')
    ) {
      const host = parsed.hostname === 'localhost' ? '127.0.0.1' : parsed.hostname;
      const port = parsed.port || '14771';
      return host.includes(':') ? `[${host}]:${port}` : `${host}:${port}`;
    }
  } catch {
    // Fall back to default loopback socket address.
  }
  return '127.0.0.1:14771';
}

function formatQuotedVaultPath(vaultPath: string): string {
  const trimmed = vaultPath.trim() || '/path/to/your/vault';
  return `"${trimmed.replace(/"/g, '\\"')}"`;
}

export function buildLocalStartCommand(vaultPath: string, endpointUrl: string): string {
  const addr = resolveLocalHttpAddr(endpointUrl);
  return `aether-mcp --vault ${formatQuotedVaultPath(vaultPath)} --http-addr ${addr} --http-only`;
}

export function buildServiceInstallCommand(vaultPath: string, endpointUrl: string): string {
  const addr = resolveLocalHttpAddr(endpointUrl);
  return `aether-mcp service install --vault ${formatQuotedVaultPath(vaultPath)} --http-addr ${addr}`;
}

const UNREACHABLE = /ERR_CONNECTION|ECONNREFUSED|ERR_NAME_NOT_RESOLVED|ENOTFOUND|Failed to fetch/i;

/** Turns a raw network failure into an actionable message; other errors pass through unchanged. */
export function describeConnectionError(err: unknown, endpointUrl: string): string {
  const detail = err instanceof Error ? err.message : String(err);
  if (UNREACHABLE.test(detail)) {
    return `Cannot reach aether-mcp at ${endpointUrl}. Is it running? Use "Local server quickstart" in the plugin settings to install and start it.`;
  }
  return detail;
}

export function plural(count: number, one: string, many = `${one}s`): string {
  return `${count} ${count === 1 ? one : many}`;
}

export type ConnectionState =
  | {kind: 'checking'}
  | {kind: 'connected'; noteCount: number}
  | {kind: 'offline'}
  | {kind: 'rejected'}
  | {kind: 'error'; message: string};

export interface ConnectionView {
  tone: 'neutral' | 'ok' | 'bad';
  title: string;
  detail: string;
}

/** Sorts a failed request into the states the settings card knows how to explain. */
export function classifyConnectionError(err: unknown): Exclude<ConnectionState, {kind: 'checking' | 'connected'}> {
  const message = err instanceof Error ? err.message : String(err);
  if (UNREACHABLE.test(message)) {
    return {kind: 'offline'};
  }
  if (/\((401|403)\)|unauthor|forbidden/i.test(message)) {
    return {kind: 'rejected'};
  }
  return {kind: 'error', message};
}

export function describeConnectionState(state: ConnectionState, endpointUrl: string): ConnectionView {
  switch (state.kind) {
    case 'checking':
      return {tone: 'neutral', title: 'Checking connection...', detail: endpointUrl};
    case 'connected':
      return {
        tone: 'ok',
        title: 'Connected to aether-mcp',
        detail: `${endpointUrl}. ${plural(state.noteCount, 'note')} indexed.`,
      };
    case 'offline':
      return {
        tone: 'bad',
        title: "Can't reach aether-mcp",
        detail: `Nothing is answering at ${endpointUrl}. Install and start it with "Local server quickstart" below, then test again.`,
      };
    case 'rejected':
      return {
        tone: 'bad',
        title: 'The server rejected the API key',
        detail: 'Check the API key below. Servers that require authentication need a key that starts with aek_.',
      };
    case 'error':
      return {tone: 'bad', title: 'Connection problem', detail: state.message};
  }
}
