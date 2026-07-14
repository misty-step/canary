import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { createClient, type CanaryClient, type EventPayload } from "../src/client";

describe("createClient", () => {
  let fetchSpy: ReturnType<typeof vi.fn>;
  const originalFetch = globalThis.fetch;

  beforeEach(() => {
    fetchSpy = vi.fn();
    globalThis.fetch = fetchSpy;
  });

  afterEach(() => {
    globalThis.fetch = originalFetch;
  });

  const opts = {
    endpoint: "https://canary.test",
    apiKey: "sk_test_abc123",
    service: "test-app",
  };

  it("sends POST to /api/v1/errors with auth header", async () => {
    fetchSpy.mockResolvedValueOnce(
      new Response(
        JSON.stringify({
          id: "ERR-abc",
          group_hash: "hash",
          is_new_class: true,
        }),
        { status: 201 }
      )
    );

    const client = createClient(opts);
    await client.send({
      error_class: "TypeError",
      message: "boom",
      severity: "error",
    });

    expect(fetchSpy).toHaveBeenCalledOnce();
    const [url, init] = fetchSpy.mock.calls[0];
    expect(url).toBe("https://canary.test/api/v1/errors");
    expect(init.method).toBe("POST");
    expect(init.headers["Authorization"]).toBe("Bearer sk_test_abc123");
    const body = JSON.parse(init.body);
    expect(body.service).toBe("test-app");
    expect(body.error_class).toBe("TypeError");
  });

  it("does not throw when endpoint is unreachable", async () => {
    fetchSpy.mockRejectedValueOnce(new Error("Network error"));

    const client = createClient(opts);
    // Should resolve without throwing
    await expect(
      client.send({
        error_class: "Error",
        message: "test",
        severity: "error",
      })
    ).resolves.toBeNull();
  });

  it("retries once on failure then gives up", async () => {
    fetchSpy
      .mockRejectedValueOnce(new Error("timeout"))
      .mockResolvedValueOnce(
        new Response(
          JSON.stringify({
            id: "ERR-xyz",
            group_hash: "h",
            is_new_class: false,
          }),
          { status: 201 }
        )
      );

    const client = createClient(opts);
    const result = await client.send({
      error_class: "Error",
      message: "retry test",
      severity: "error",
    });

    expect(fetchSpy).toHaveBeenCalledTimes(2);
    expect(result).toEqual({
      id: "ERR-xyz",
      group_hash: "h",
      is_new_class: false,
    });
  });

  it("returns null after both retry attempts fail", async () => {
    fetchSpy
      .mockRejectedValueOnce(new Error("timeout"))
      .mockRejectedValueOnce(new Error("still down"));

    const client = createClient(opts);

    await expect(
      client.send({
        error_class: "Error",
        message: "retry exhausted",
        severity: "error",
      })
    ).resolves.toBeNull();

    expect(fetchSpy).toHaveBeenCalledTimes(2);
  });

  it("returns null when the server responds with a non-2xx status", async () => {
    fetchSpy.mockResolvedValueOnce(new Response("bad gateway", { status: 502 }));

    const client = createClient(opts);
    await expect(
      client.send({
        error_class: "Error",
        message: "server failure",
        severity: "error",
      })
    ).resolves.toBeNull();
  });

  it("retries once on transient HTTP status", async () => {
    fetchSpy
      .mockResolvedValueOnce(new Response("bad gateway", { status: 502 }))
      .mockResolvedValueOnce(
        new Response(
          JSON.stringify({
            id: "ERR-retried",
            group_hash: "retry-hash",
            is_new_class: false,
          }),
          { status: 201 }
        )
      );

    const client = createClient(opts);
    const result = await client.send({
      error_class: "Error",
      message: "server retry",
      severity: "error",
    });

    expect(fetchSpy).toHaveBeenCalledTimes(2);
    expect(result).toEqual({
      id: "ERR-retried",
      group_hash: "retry-hash",
      is_new_class: false,
    });
  });

  it("returns null when inflight reaches maxQueue", async () => {
    // Never resolve — simulate slow network
    fetchSpy.mockImplementation(
      () => new Promise<Response>(() => {})
    );

    const client = createClient({ ...opts, maxQueue: 3 });

    // Fire 3 sends to fill the queue
    client.send({ error_class: "E1", message: "1", severity: "error" });
    client.send({ error_class: "E2", message: "2", severity: "error" });
    client.send({ error_class: "E3", message: "3", severity: "error" });

    // 4th should be rejected
    const overflow = client.send({
      error_class: "E4",
      message: "4",
      severity: "error",
    });

    expect(client.pending).toBe(3);
    await expect(overflow).resolves.toBeNull();
  });

  it("strips trailing slash from endpoint", async () => {
    fetchSpy.mockResolvedValueOnce(
      new Response(JSON.stringify({ id: "ERR-1", group_hash: "h", is_new_class: false }), {
        status: 201,
      })
    );

    const client = createClient({ ...opts, endpoint: "https://canary.test/" });
    await client.send({
      error_class: "Error",
      message: "test",
      severity: "error",
    });

    const [url] = fetchSpy.mock.calls[0];
    expect(url).toBe("https://canary.test/api/v1/errors");
  });

  it("includes environment and context in payload", async () => {
    fetchSpy.mockResolvedValueOnce(
      new Response(JSON.stringify({ id: "ERR-1", group_hash: "h", is_new_class: false }), {
        status: 201,
      })
    );

    const client = createClient({ ...opts, environment: "staging" });
    await client.send({
      error_class: "Error",
      message: "test",
      severity: "warning",
      stack_trace: "at foo:1",
      context: { userId: "u1" },
    });

    const body = JSON.parse(fetchSpy.mock.calls[0][1].body);
    expect(body.environment).toBe("staging");
    expect(body.severity).toBe("warning");
    expect(body.stack_trace).toBe("at foo:1");
    expect(body.context).toEqual({ userId: "u1" });
  });

  it("sends monitor check-ins to /api/v1/check-ins", async () => {
    fetchSpy.mockResolvedValueOnce(
      new Response(
        JSON.stringify({
          monitor_id: "MON-1",
          check_in_id: "CHK-1",
          state: "up",
          observed_at: "2026-06-14T00:00:00Z",
          sequence: 1,
        }),
        { status: 201 }
      )
    );

    const client = createClient({ ...opts, environment: "production" });
    const result = await client.checkIn({
      monitor: "test-app-cron",
      status: "ok",
      check_in_id: "run-1",
      summary: "nightly job complete",
      context: { duration_ms: 1200 },
    });

    expect(result?.monitor_id).toBe("MON-1");
    const [url, init] = fetchSpy.mock.calls[0];
    expect(url).toBe("https://canary.test/api/v1/check-ins");
    expect(init.headers["Authorization"]).toBe("Bearer sk_test_abc123");
    const body = JSON.parse(init.body);
    expect(body.service).toBe("test-app");
    expect(body.monitor).toBe("test-app-cron");
    expect(body.status).toBe("ok");
    expect(body.context).toEqual({ duration_ms: 1200 });
  });

  it("sends analytics events to /api/v1/events with safe defaults", async () => {
    fetchSpy.mockResolvedValueOnce(
      new Response(
        JSON.stringify({
          id: "EVT-telemetry01",
          service: "test-app",
          event: "telemetry.event",
          name: "checkout.completed",
          severity: "info",
          summary: "Checkout completed",
          attributes: { plan: "pro" },
          retention_class: "standard",
          privacy_policy: "redacted",
          sampling_policy: "unsampled",
          created_at: "2026-06-14T00:00:00Z",
        }),
        { status: 201 }
      )
    );

    const client = createClient(opts);
    const result = await client.event({
      name: "checkout.completed",
      summary: "Checkout completed",
      attributes: { plan: "pro" },
    });

    expect(fetchSpy).toHaveBeenCalledOnce();
    const [url, init] = fetchSpy.mock.calls[0];
    expect(url).toBe("https://canary.test/api/v1/events");
    const body = JSON.parse(init.body);
    expect(body.service).toBe("test-app");
    expect(body.name).toBe("checkout.completed");
    expect(body.retention_class).toBe("standard");
    expect(body.privacy_policy).toBe("redacted");
    expect(body.sampling_policy).toBe("unsampled");
    expect(result?.event).toBe("telemetry.event");
  });

  it("sends operational events as bounded audit signals", async () => {
    fetchSpy.mockResolvedValueOnce(
      new Response(
        JSON.stringify({
          id: "EVT-operational01",
          service: "test-app",
          event: "telemetry.event",
          name: "capacity.saturation",
          severity: "warning",
          summary: "Capacity saturated",
          attributes: {},
          retention_class: "audit",
          privacy_policy: "redacted",
          sampling_policy: "unsampled",
          created_at: "2026-07-14T14:01:01Z",
          incident_event: "incident.opened",
          incident_id: "INC-operational01",
        }),
        { status: 201 }
      )
    );

    const client = createClient(opts);
    await client.event({
      name: "capacity.saturation",
      summary: "Capacity saturated",
      severity: "warning",
      operational: {
        subject: { type: "capacity", id: "worker-pool" },
        state: "active",
        owner: "infrastructure-operator",
        evidence_url: "https://evidence.example/receipts/capacity",
        observed_at: "2026-07-14T14:01:00Z",
      },
    });

    const [, init] = fetchSpy.mock.calls[0];
    const body = JSON.parse(init.body);
    expect(body.retention_class).toBe("audit");
    expect(body.attributes).toEqual({});
    expect(body.operational.subject).toEqual({ type: "capacity", id: "worker-pool" });
    expect(body.operational.evidence_url).toBe(
      "https://evidence.example/receipts/capacity"
    );
  });

  it("rejects conflicting or unbounded operational payloads before transport", async () => {
    const client = createClient(opts);
    const base = {
      name: "capacity.saturation",
      summary: "Capacity saturated",
      operational: {
        subject: { type: "capacity", id: "worker-pool" },
        state: "active",
        owner: "infrastructure-operator",
        evidence_url: "https://evidence.example/receipts/capacity",
        observed_at: "2026-07-14T14:01:00Z",
      },
    };
    for (const invalid of [
      { ...base, attributes: { raw_metrics: [1, 2, 3] } },
      { ...base, retention_class: "standard" },
      { ...base, privacy_policy: "public" },
      { ...base, sampling_policy: "sampled:0.5" },
      { ...base, provider_snapshot: { droplets: [1, 2, 3] } },
      { ...base, operational: null },
      { ...base, operational: [] },
      { ...base, operational: { ...base.operational, samples: [1, 2, 3] } },
      { ...base, operational: { ...base.operational, subject: null } },
      { ...base, operational: { ...base.operational, subject: [] } },
      {
        ...base,
        operational: {
          ...base.operational,
          subject: { ...base.operational.subject, provider: "private" },
        },
      },
      {
        ...base,
        operational: {
          ...base.operational,
          subject: { ...base.operational.subject, type: "Invalid" },
        },
      },
      {
        ...base,
        operational: {
          ...base.operational,
          subject: { ...base.operational.subject, id: "invalid/id" },
        },
      },
      { ...base, operational: { ...base.operational, state: "unknown" } },
      { ...base, operational: { ...base.operational, owner: "" } },
      { ...base, operational: { ...base.operational, owner: "x".repeat(129) } },
      { ...base, operational: { ...base.operational, evidence_url: 42 } },
      { ...base, operational: { ...base.operational, evidence_url: "x".repeat(2049) } },
      { ...base, operational: { ...base.operational, evidence_url: "not a url" } },
      { ...base, operational: { ...base.operational, evidence_url: "http://unsafe.test" } },
      {
        ...base,
        operational: {
          ...base.operational,
          evidence_url: "https://user:password@evidence.example/receipt",
        },
      },
      { ...base, operational: { ...base.operational, observed_at: 42 } },
      { ...base, operational: { ...base.operational, observed_at: "not-a-clock" } },
      {
        ...base,
        operational: {
          ...base.operational,
          observed_at: "2026-99-99T14:01:00Z",
        },
      },
    ]) {
      await expect(
        client.event(invalid as unknown as EventPayload)
      ).resolves.toBeNull();
    }
    expect(fetchSpy).not.toHaveBeenCalled();
  });
});
