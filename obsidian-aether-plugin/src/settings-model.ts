import {DEFAULT_CONTROL_PLANE_URL} from './api.ts';

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
  endpointUrl: DEFAULT_CONTROL_PLANE_URL,
  apiKey: '',
  liveSyncEnabled: true,
  syncDebounceMs: 1500,
  autoPullIntervalSec: 0,
  runDreamOnPush: false,
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
