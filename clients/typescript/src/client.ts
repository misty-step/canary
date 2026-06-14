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

export interface CheckInPayload {
  monitor: string;
  status: "alive" | "in_progress" | "ok" | "error";
  check_in_id?: string;
  observed_at?: string;
  ttl_ms?: number;
  summary?: string;
  context?: Record<string, unknown>;
}

export interface EventPayload {
  name: string;
  summary: string;
  severity?: "info" | "warning" | "error";
  attributes?: Record<string, unknown>;
  retention_class?: "ephemeral" | "standard" | "audit";
  privacy_policy?: "redacted" | "public" | "sensitive";
  sampling_policy?: string;
}

export interface CanaryResponse {
  id: string;
  group_hash: string;
  is_new_class: boolean;
}

export interface CheckInResponse {
  monitor_id: string;
  check_in_id: string;
  state: string;
  observed_at: string;
  sequence: number;
}

export interface EventResponse {
  id: string;
  service: string;
  event: "telemetry.event";
  name: string;
  severity: "info" | "warning" | "error";
  summary: string;
  attributes: Record<string, unknown>;
  retention_class: "ephemeral" | "standard" | "audit";
  privacy_policy: "redacted" | "public" | "sensitive";
  sampling_policy: string;
  created_at: string;
}

export interface CanaryClient {
  send(payload: ErrorPayload): Promise<CanaryResponse | null>;
  checkIn(payload: CheckInPayload): Promise<CheckInResponse | null>;
  event(payload: EventPayload): Promise<EventResponse | null>;
  readonly pending: number;
}

export function createClient(opts: ClientOptions): CanaryClient {
  const endpoint = opts.endpoint.replace(/\/$/, "");
  const errorsUrl = `${endpoint}/api/v1/errors`;
  const eventsUrl = `${endpoint}/api/v1/events`;
  const checkInsUrl = `${endpoint}/api/v1/check-ins`;
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
      return await attempt<CanaryResponse>(errorsUrl, opts.apiKey, body, 1);
    } catch {
      return null;
    } finally {
      inflight--;
    }
  }

  async function checkIn(
    payload: CheckInPayload
  ): Promise<CheckInResponse | null> {
    if (inflight >= maxQueue) return null;
    inflight++;

    const body = JSON.stringify({
      service: opts.service,
      environment: opts.environment ?? "production",
      ...payload,
    });

    try {
      return await attempt<CheckInResponse>(checkInsUrl, opts.apiKey, body, 1);
    } catch {
      return null;
    } finally {
      inflight--;
    }
  }

  async function event(payload: EventPayload): Promise<EventResponse | null> {
    if (inflight >= maxQueue) return null;
    inflight++;

    const body = JSON.stringify({
      service: opts.service,
      severity: "info",
      retention_class: "standard",
      privacy_policy: "redacted",
      sampling_policy: "unsampled",
      ...payload,
    });

    try {
      return await attempt<EventResponse>(eventsUrl, opts.apiKey, body, 1);
    } catch {
      return null;
    } finally {
      inflight--;
    }
  }

  return {
    send,
    checkIn,
    event,
    get pending() {
      return inflight;
    },
  };
}

async function attempt<T>(
  url: string,
  apiKey: string,
  body: string,
  retries: number
): Promise<T | null> {
  try {
    const res = await fetch(url, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Authorization: `Bearer ${apiKey}`,
      },
      body,
      signal:
        typeof AbortSignal.timeout === "function"
          ? AbortSignal.timeout(2_000)
          : undefined,
    });
    if (!res.ok) {
      if (retries > 0 && isTransientStatus(res.status)) {
        return attempt<T>(url, apiKey, body, retries - 1);
      }
      return null;
    }
    return (await res.json()) as T;
  } catch (err) {
    if (retries > 0) return attempt<T>(url, apiKey, body, retries - 1);
    throw err;
  }
}

function isTransientStatus(status: number): boolean {
  return status === 408 || status === 429 || (status >= 500 && status <= 599);
}
