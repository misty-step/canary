import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { initCanary, captureException, captureMessage } from "../src/index";

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

  it("applies PII scrubbing when enabled", async () => {
    initCanary({
      endpoint: "https://canary.test",
      apiKey: "sk_test_abc",
      service: "test-svc",
      scrubPii: true,
    });

    await captureException(new Error("user alice@example.com failed"));

    const body = JSON.parse(fetchSpy.mock.calls[0][1].body);
    expect(body.message).toBe("user [EMAIL] failed");
    expect(body.message).not.toContain("alice@example.com");
  });

  it("scrubs PII in context when enabled", async () => {
    initCanary({
      endpoint: "https://canary.test",
      apiKey: "sk_test_abc",
      service: "test-svc",
      scrubPii: true,
    });

    await captureException(new Error("oops"), {
      context: { userEmail: "alice@example.com", count: 5 },
    });

    const body = JSON.parse(fetchSpy.mock.calls[0][1].body);
    expect(body.context.userEmail).toBe("[EMAIL]");
    expect(body.context.count).toBe(5);
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

describe("uninitialized capture helpers", () => {
  it("return null before initCanary runs", async () => {
    vi.resetModules();
    const { captureException, captureMessage } = await import("../src/index");

    await expect(captureException(new Error("boom"))).resolves.toBeNull();
    await expect(captureMessage("boom")).resolves.toBeNull();
  });
});
