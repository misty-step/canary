export interface ClientOptions {
  endpoint: string;
  apiKey: string;
  service: string;
  environment?: string;
  maxQueue?: number;
}

export interface ErrorPayload {
  error_class: string;
  message: string;
  severity: "error" | "warning" | "info";
  stack_trace?: string;
  context?: Record<string, unknown>;
  fingerprint?: string[];
}

export interface CanaryResponse {
  id: string;
  group_hash: string;
  is_new_class: boolean;
}

export interface CanaryClient {
  send(payload: ErrorPayload): Promise<CanaryResponse | null>;
  readonly pending: number;
}

export function createClient(opts: ClientOptions): CanaryClient {
  const endpoint = opts.endpoint.replace(/\/$/, "");
  const url = `${endpoint}/api/v1/errors`;
  const maxQueue = opts.maxQueue ?? 10;
  let inflight = 0;

  async function send(payload: ErrorPayload): Promise<CanaryResponse | null> {
    if (inflight >= maxQueue) return null;
    inflight++;

    const body = JSON.stringify({
      service: opts.service,
      environment: opts.environment ?? "production",
      ...payload,
    });

    try {
      return await attempt(url, opts.apiKey, body, 1);
    } catch {
      return null;
    } finally {
      inflight--;
    }
  }

  return {
    send,
    get pending() {
      return inflight;
    },
  };
}

async function attempt(
  url: string,
  apiKey: string,
  body: string,
  retries: number
): Promise<CanaryResponse | null> {
  try {
    const res = await fetch(url, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Authorization: `Bearer ${apiKey}`,
      },
      body,
      signal: AbortSignal.timeout(2_000),
    });
    if (!res.ok) return null;
    return (await res.json()) as CanaryResponse;
  } catch (err) {
    if (retries > 0) return attempt(url, apiKey, body, retries - 1);
    throw err;
  }
}
