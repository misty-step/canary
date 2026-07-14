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

interface EventPayloadBase {
  name: string;
  summary: string;
  severity?: "info" | "warning" | "error";
}

export interface AnalyticsEventPayload extends EventPayloadBase {
  attributes?: Record<string, unknown>;
  retention_class?: "ephemeral" | "standard" | "audit";
  privacy_policy?: "redacted" | "public" | "sensitive";
  sampling_policy?: string;
  operational?: never;
}

export interface OperationalEventPayload extends EventPayloadBase {
  operational: OperationalSignal;
  attributes?: never;
  retention_class?: "audit";
  privacy_policy?: "redacted";
  sampling_policy?: "unsampled";
}

export type EventPayload = AnalyticsEventPayload | OperationalEventPayload;

export interface OperationalSignal {
  subject: {
    type: string;
    id: string;
  };
  state: "active" | "resolved";
  owner: string;
  evidence_url: string;
  observed_at: string;
}

export interface OperationalSignalContext {
  name: string;
  subject_type: string;
  subject_id: string;
  state: "active" | "resolved";
  owner: string;
  evidence_url: string;
  observed_at: string;
  received_at: string;
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
  operational?: OperationalSignalContext;
  incident_event?: "incident.opened" | "incident.updated" | "incident.resolved";
  incident_id?: string;
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
    const normalized = normalizeEventPayload(payload);
    if (!normalized) return null;
    inflight++;

    const body = JSON.stringify({
      service: opts.service,
      ...normalized,
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

function normalizeEventPayload(payload: EventPayload): Record<string, unknown> | null {
  const object = payload as unknown as Record<string, unknown>;
  const allowed = new Set([
    "name",
    "summary",
    "severity",
    "attributes",
    "retention_class",
    "privacy_policy",
    "sampling_policy",
    "operational",
  ]);
  if (Object.keys(object).some((field) => !allowed.has(field))) return null;

  if (!Object.prototype.hasOwnProperty.call(object, "operational")) {
    return {
      severity: "info",
      retention_class: "standard",
      privacy_policy: "redacted",
      sampling_policy: "unsampled",
      ...payload,
    };
  }

  if (
    object.attributes !== undefined ||
    (object.retention_class !== undefined && object.retention_class !== "audit") ||
    (object.privacy_policy !== undefined && object.privacy_policy !== "redacted") ||
    (object.sampling_policy !== undefined && object.sampling_policy !== "unsampled") ||
    !validOperationalSignal(object.operational)
  ) {
    return null;
  }

  return {
    severity: "info",
    ...payload,
    attributes: {},
    retention_class: "audit",
    privacy_policy: "redacted",
    sampling_policy: "unsampled",
  };
}

function validOperationalSignal(value: unknown): value is OperationalSignal {
  if (!value || typeof value !== "object" || Array.isArray(value)) return false;
  const object = value as Record<string, unknown>;
  if (
    Object.keys(object).some(
      (field) => !["subject", "state", "owner", "evidence_url", "observed_at"].includes(field)
    )
  ) return false;
  if (!object.subject || typeof object.subject !== "object" || Array.isArray(object.subject)) {
    return false;
  }
  const subject = object.subject as Record<string, unknown>;
  if (Object.keys(subject).some((field) => !["type", "id"].includes(field))) return false;
  if (typeof subject.type !== "string" || !/^[a-z0-9._-]{1,64}$/.test(subject.type)) return false;
  if (typeof subject.id !== "string" || !/^[A-Za-z0-9._:-]{1,160}$/.test(subject.id)) return false;
  if (object.state !== "active" && object.state !== "resolved") return false;
  if (typeof object.owner !== "string" || object.owner.trim().length === 0 || [...object.owner].length > 128) return false;
  if (typeof object.evidence_url !== "string" || [...object.evidence_url].length > 2048) return false;
  try {
    const evidence = new URL(object.evidence_url);
    if (evidence.protocol !== "https:" || evidence.username || evidence.password || !evidence.hostname) {
      return false;
    }
  } catch {
    return false;
  }
  return typeof object.observed_at === "string" && /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})$/.test(
    object.observed_at
  ) && !Number.isNaN(Date.parse(object.observed_at));
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
