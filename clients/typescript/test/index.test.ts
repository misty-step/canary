import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { initCanary, captureException, captureMessage, checkIn, captureEvent } from "../src/index";

describe("initCanary + captureException", () => {
  let fetchSpy: ReturnType<typeof vi.fn>;
  const originalFetch = globalThis.fetch;

  const ok = () =>
    new Response(
      JSON.stringify({ id: "ERR-1", group_hash: "h", is_new_class: false }),
      { status: 201 }
    );

  beforeEach(() => {
    fetchSpy = vi.fn().mockResolvedValue(ok());
    globalThis.fetch = fetchSpy;
  });

  afterEach(() => {
    globalThis.fetch = originalFetch;
  });

  it("sends error with class, message, and stack trace", async () => {
    initCanary({
      endpoint: "https://canary.test",
      apiKey: "sk_test_abc",
      service: "test-svc",
    });

    const err = new TypeError("cannot read property x");
    await captureException(err);

    expect(fetchSpy).toHaveBeenCalledOnce();
    const body = JSON.parse(fetchSpy.mock.calls[0][1].body);
    expect(body.error_class).toBe("TypeError");
    expect(body.message).toBe("cannot read property x");
    expect(body.stack_trace).toBeDefined();
    expect(body.service).toBe("test-svc");
  });

  it("handles non-Error objects gracefully", async () => {
    initCanary({
      endpoint: "https://canary.test",
      apiKey: "sk_test_abc",
      service: "test-svc",
    });

    await captureException("string error");

    const body = JSON.parse(fetchSpy.mock.calls[0][1].body);
    expect(body.error_class).toBe("StringError");
    expect(body.message).toBe("string error");
  });

  it("handles unknown error types", async () => {
    initCanary({
      endpoint: "https://canary.test",
      apiKey: "sk_test_abc",
      service: "test-svc",
    });

    await captureException(42);

    const body = JSON.parse(fetchSpy.mock.calls[0][1].body);
    expect(body.error_class).toBe("UnknownError");
    expect(body.message).toBe("42");
  });

  it("fails silently when endpoint is unreachable", async () => {
    fetchSpy.mockRejectedValue(new Error("network down"));

    initCanary({
      endpoint: "https://canary.test",
      apiKey: "sk_test_abc",
      service: "test-svc",
    });

    // Must not throw
    await expect(captureException(new Error("boom"))).resolves.toBeNull();
  });

  it("applies PII scrubbing by default", async () => {
    initCanary({
      endpoint: "https://canary.test",
      apiKey: "sk_test_abc",
      service: "test-svc",
    });

    await captureException(new Error("user alice@example.com failed"));

    const body = JSON.parse(fetchSpy.mock.calls[0][1].body);
    expect(body.message).toBe("user [EMAIL] failed");
    expect(body.message).not.toContain("alice@example.com");
  });

  it("scrubs PII in context by default", async () => {
    initCanary({
      endpoint: "https://canary.test",
      apiKey: "sk_test_abc",
      service: "test-svc",
    });

    await captureException(new Error("oops"), {
      context: { userEmail: "alice@example.com", count: 5 },
    });

    const body = JSON.parse(fetchSpy.mock.calls[0][1].body);
    expect(body.context.userEmail).toBe("[EMAIL]");
    expect(body.context.count).toBe(5);
  });

  it("allows explicit PII scrubbing opt-out", async () => {
    initCanary({
      endpoint: "https://canary.test",
      apiKey: "sk_test_abc",
      service: "test-svc",
      scrubPii: false,
    });

    await captureException(new Error("user alice@example.com failed"));

    const body = JSON.parse(fetchSpy.mock.calls[0][1].body);
    expect(body.message).toBe("user alice@example.com failed");
  });

  it("passes context and severity through", async () => {
    initCanary({
      endpoint: "https://canary.test",
      apiKey: "sk_test_abc",
      service: "test-svc",
    });

    await captureException(new Error("oops"), {
      severity: "warning",
      context: { route: "/api/foo" },
    });

    const body = JSON.parse(fetchSpy.mock.calls[0][1].body);
    expect(body.severity).toBe("warning");
    expect(body.context).toEqual({ route: "/api/foo" });
  });
});

describe("captureMessage", () => {
  let fetchSpy: ReturnType<typeof vi.fn>;
  const originalFetch = globalThis.fetch;

  beforeEach(() => {
    fetchSpy = vi.fn().mockResolvedValue(
      new Response(
        JSON.stringify({ id: "ERR-1", group_hash: "h", is_new_class: false }),
        { status: 201 }
      )
    );
    globalThis.fetch = fetchSpy;
  });

  afterEach(() => {
    globalThis.fetch = originalFetch;
  });

  it("sends a message-only error report", async () => {
    initCanary({
      endpoint: "https://canary.test",
      apiKey: "sk_test_abc",
      service: "test-svc",
    });

    await captureMessage("something happened", { severity: "info" });

    const body = JSON.parse(fetchSpy.mock.calls[0][1].body);
    expect(body.error_class).toBe("Message");
    expect(body.message).toBe("something happened");
    expect(body.severity).toBe("info");
  });

  it("scrubs message context when enabled", async () => {
    initCanary({
      endpoint: "https://canary.test",
      apiKey: "sk_test_abc",
      service: "test-svc",
      scrubPii: true,
    });

    await captureMessage("contact alice@example.com", {
      context: { owner: "alice@example.com" },
    });

    const body = JSON.parse(fetchSpy.mock.calls[0][1].body);
    expect(body.message).toBe("contact [EMAIL]");
    expect(body.context).toEqual({ owner: "[EMAIL]" });
  });
});

describe("checkIn", () => {
  let fetchSpy: ReturnType<typeof vi.fn>;
  const originalFetch = globalThis.fetch;

  beforeEach(() => {
    fetchSpy = vi.fn().mockResolvedValue(
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
    globalThis.fetch = fetchSpy;
  });

  afterEach(() => {
    globalThis.fetch = originalFetch;
  });

  it("sends scrubbed monitor check-ins", async () => {
    initCanary({
      endpoint: "https://canary.test",
      apiKey: "sk_test_abc",
      service: "test-svc",
    });

    await checkIn({
      monitor: "test-svc-cron",
      status: "ok",
      summary: "job complete for alice@example.com",
      context: { owner: "alice@example.com" },
    });

    const [url, init] = fetchSpy.mock.calls[0];
    expect(url).toBe("https://canary.test/api/v1/check-ins");
    const body = JSON.parse(init.body);
    expect(body.monitor).toBe("test-svc-cron");
    expect(body.summary).toBe("job complete for [EMAIL]");
    expect(body.context).toEqual({ owner: "[EMAIL]" });
  });
});

describe("captureEvent", () => {
  let fetchSpy: ReturnType<typeof vi.fn>;
  const originalFetch = globalThis.fetch;

  beforeEach(() => {
    fetchSpy = vi.fn().mockResolvedValue(
      new Response(
        JSON.stringify({
          id: "EVT-1",
          service: "test-svc",
          event: "telemetry.event",
          name: "signup.completed",
          severity: "info",
          summary: "Signup completed",
          attributes: { email: "[EMAIL]" },
          retention_class: "standard",
          privacy_policy: "redacted",
          sampling_policy: "unsampled",
          created_at: "2026-06-14T00:00:00Z",
        }),
        { status: 201 }
      )
    );
    globalThis.fetch = fetchSpy;
  });

  afterEach(() => {
    globalThis.fetch = originalFetch;
  });

  it("sends scrubbed analytics events", async () => {
    initCanary({
      endpoint: "https://canary.test",
      apiKey: "sk_test_abc",
      service: "test-svc",
    });

    await captureEvent({
      name: "signup.completed",
      summary: "Signup completed for alice@example.com",
      attributes: { email: "alice@example.com", plan: "pro" },
    });

    const body = JSON.parse(fetchSpy.mock.calls[0][1].body);
    expect(fetchSpy.mock.calls[0][0]).toBe("https://canary.test/api/v1/events");
    expect(body.service).toBe("test-svc");
    expect(body.name).toBe("signup.completed");
    expect(body.summary).toBe("Signup completed for [EMAIL]");
    expect(body.attributes).toEqual({ email: "[EMAIL]", plan: "pro" });
  });

  it("preserves the discriminated operational envelope without analytics attributes", async () => {
    initCanary({
      endpoint: "https://canary.test",
      apiKey: "sk_test_abc",
      service: "test-svc",
    });

    await captureEvent({
      name: "drift.violation",
      summary: "Drift detected",
      operational: {
        subject: { type: "deployment", id: "production" },
        state: "active",
        owner: "infrastructure-operator",
        evidence_url: "https://evidence.example/receipts/drift",
        observed_at: "2026-07-14T14:01:00Z",
      },
    });

    const body = JSON.parse(fetchSpy.mock.calls[0][1].body);
    expect(body.attributes).toEqual({});
    expect(body.retention_class).toBe("audit");
    expect(body.operational.subject).toEqual({
      type: "deployment",
      id: "production",
    });
  });
});

describe("uninitialized capture helpers", () => {
  it("return null before initCanary runs", async () => {
    vi.resetModules();
    const { captureException, captureMessage, checkIn, captureEvent } = await import("../src/index");

    await expect(captureException(new Error("boom"))).resolves.toBeNull();
    await expect(captureMessage("boom")).resolves.toBeNull();
    await expect(
      checkIn({ monitor: "test", status: "alive" })
    ).resolves.toBeNull();
    await expect(
      captureEvent({ name: "test.event", summary: "test" })
    ).resolves.toBeNull();
  });
});
