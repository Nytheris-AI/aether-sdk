import type {
  DecideResponse,
  DreamReport,
  GitSyncOptionsRequest,
  GitSyncReport,
  InboxDecision,
  InboxListResponse,
  KvCacheStats,
  KvWarmReport,
  SyncReport,
  VaultPullResponse,
  VaultPushRequest,
  VaultPushResponse,
  VaultStatusReport,
  WarmKvRequest,
} from './types.ts';

const LOOPBACK_HOSTS = new Set(['127.0.0.1', 'localhost', '[::1]', '::1']);

export const DEFAULT_CONTROL_PLANE_URL = 'http://127.0.0.1:14771';

/**
 * Validates that the companion control-plane URL points strictly to loopback HTTP.
 */
export function assertLoopbackBaseUrl(rawUrl: string): string {
  const parsed = new URL(rawUrl);
  if (parsed.protocol !== 'http:') {
    throw new Error(`Aether control plane requires http:// loopback URL (got ${parsed.protocol})`);
  }
  if (!LOOPBACK_HOSTS.has(parsed.hostname.toLowerCase())) {
    throw new Error(
      `Aether control plane must bind strictly to loopback, refusing host "${parsed.hostname}"`,
    );
  }
  return parsed.origin;
}

export interface BaseUrlValidationOptions {
  apiKey?: string;
  allowInsecureRemoteHttp?: boolean;
}

/**
 * Validates loopback or authenticated remote team control-plane URLs.
 * Remote endpoints require `apiKey` and default to `https://` unless `allowInsecureRemoteHttp` is set.
 */
export function assertControlPlaneBaseUrl(
  rawUrl: string,
  options: BaseUrlValidationOptions = {},
): string {
  const parsed = new URL(rawUrl);
  const isLoopback = LOOPBACK_HOSTS.has(parsed.hostname.toLowerCase());
  const hasApiKey = Boolean(options.apiKey && options.apiKey.trim().length > 0);

  if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') {
    throw new Error(`Aether control plane requires http:// or https:// URL (got ${parsed.protocol})`);
  }
  if (isLoopback) {
    if (parsed.protocol === 'https:' && !hasApiKey) {
      throw new Error('Aether HTTPS endpoint requires an API key');
    }
    return parsed.origin;
  }
  if (!hasApiKey) {
    throw new Error(
      `Aether control plane refuses remote host "${parsed.hostname}" without a bearer apiKey`,
    );
  }
  if (parsed.protocol !== 'https:' && !options.allowInsecureRemoteHttp) {
    throw new Error(
      `Aether remote team endpoint "${parsed.hostname}" requires https:// unless allowInsecureRemoteHttp is enabled`,
    );
  }
  return parsed.origin;
}

export type FetchLike = typeof fetch;

export interface AetherClientOptions {
  fetchImpl?: FetchLike;
  apiKey?: string;
  allowInsecureRemoteHttp?: boolean;
  defaultKvModel?: string;
  runDreamOnPush?: boolean;
  kvWarmingEnabled?: boolean;
}

/**
 * HTTP client communicating with `aether-mcp --http-addr`.
 */
export class AetherControlClient {
  readonly baseUrl: string;
  private readonly fetchImpl: FetchLike;
  private readonly apiKey: string | null;

  constructor(
    baseUrl = DEFAULT_CONTROL_PLANE_URL,
    fetchOrOptions: FetchLike | AetherClientOptions = fetch,
  ) {
    const opts: AetherClientOptions =
      typeof fetchOrOptions === 'function' ? {fetchImpl: fetchOrOptions} : fetchOrOptions;
    const trimmedKey = opts.apiKey?.trim() || undefined;
    this.baseUrl = assertControlPlaneBaseUrl(baseUrl, {
      apiKey: trimmedKey,
      allowInsecureRemoteHttp: opts.allowInsecureRemoteHttp,
    });
    this.fetchImpl = opts.fetchImpl ?? fetch;
    this.apiKey = trimmedKey ?? null;
  }

  async getStatus(): Promise<VaultStatusReport> {
    return this.request<VaultStatusReport>('GET', '/v1/status');
  }

  async getInbox(): Promise<InboxListResponse> {
    return this.request<InboxListResponse>('GET', '/v1/inbox');
  }

  async decideCandidate(candidateId: string, decision: InboxDecision): Promise<DecideResponse> {
    return this.request<DecideResponse>('POST', '/v1/inbox/decide', {
      candidate_id: candidateId,
      decision,
    });
  }

  async triggerDreamPass(): Promise<DreamReport> {
    return this.request<DreamReport>('POST', '/v1/dream', {});
  }

  async syncVault(): Promise<SyncReport> {
    return this.request<SyncReport>('POST', '/v1/sync', {});
  }

  async gitSync(options: GitSyncOptionsRequest = {}): Promise<GitSyncReport> {
    return this.request<GitSyncReport>('POST', '/v1/git/sync', options);
  }

  async pullVaultState(): Promise<VaultPullResponse> {
    return this.request<VaultPullResponse>('GET', '/v1/vault/pull');
  }

  async pushVaultChanges(req: VaultPushRequest = {}): Promise<VaultPushResponse> {
    return this.request<VaultPushResponse>('POST', '/v1/vault/push', req);
  }

  async warmKvCache(req: WarmKvRequest): Promise<KvWarmReport> {
    return this.request<KvWarmReport>('POST', '/v1/kv/warm', req);
  }

  async getKvStats(modelId?: string): Promise<KvCacheStats> {
    const query =
      modelId && modelId.trim().length > 0
        ? `?model_id=${encodeURIComponent(modelId.trim())}`
        : '';
    return this.request<KvCacheStats>('GET', `/v1/kv/stats${query}`);
  }

  private async request<T>(method: 'GET' | 'POST', path: string, body?: unknown): Promise<T> {
    const headers: Record<string, string> = {};
    if (body !== undefined) {
      headers['Content-Type'] = 'application/json';
    }
    if (this.apiKey !== null) {
      headers['Authorization'] = `Bearer ${this.apiKey}`;
    }

    const res = await this.fetchImpl(`${this.baseUrl}${path}`, {
      method,
      headers: Object.keys(headers).length > 0 ? headers : undefined,
      body: body !== undefined ? JSON.stringify(body) : undefined,
    });
    const text = await res.text();
    const parsed: unknown = text.length > 0 ? JSON.parse(text) : {};
    if (!res.ok) {
      const errField =
        typeof parsed === 'object' &&
        parsed !== null &&
        'error' in parsed &&
        typeof parsed.error === 'string'
          ? parsed.error
          : `HTTP ${res.status}`;
      throw new Error(`Aether control plane error (${res.status}): ${errField}`);
    }
    return parsed as T;
  }
}
