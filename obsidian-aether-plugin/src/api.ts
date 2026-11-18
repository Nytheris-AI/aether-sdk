import type {
  DecideResponse,
  DreamReport,
  InboxDecision,
  InboxListResponse,
  SyncReport,
  VaultStatusReport,
} from './types.ts';

const LOOPBACK_HOSTS = new Set(['127.0.0.1', 'localhost', '[::1]', '::1']);

/**
 * Validates that the companion control-plane URL points strictly to loopback.
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

export type FetchLike = typeof fetch;

/**
 * HTTP client communicating with `aether-mcp --http-addr 127.0.0.1:<port>`.
 */
export class AetherControlClient {
  readonly baseUrl: string;
  private readonly fetchImpl: FetchLike;

  constructor(baseUrl = 'http://127.0.0.1:4771', fetchImpl: FetchLike = fetch) {
    this.baseUrl = assertLoopbackBaseUrl(baseUrl);
    this.fetchImpl = fetchImpl;
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

  private async request<T>(method: 'GET' | 'POST', path: string, body?: unknown): Promise<T> {
    const res = await this.fetchImpl(`${this.baseUrl}${path}`, {
      method,
      headers: body !== undefined ? {'Content-Type': 'application/json'} : undefined,
      body: body !== undefined ? JSON.stringify(body) : undefined,
    });
    const text = await res.text();
    const parsed = text.length > 0 ? (JSON.parse(text) as Record<string, unknown>) : {};
    if (!res.ok) {
      const msg = typeof parsed.error === 'string' ? parsed.error : `HTTP ${res.status}`;
      throw new Error(`Aether control plane error (${res.status}): ${msg}`);
    }
    return parsed as unknown as T;
  }
}
