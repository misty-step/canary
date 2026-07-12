import { afterEach, describe, expect, it, vi } from "vitest";
import { reportCanaryError } from "../../../crates/canary-cli/src/canary_adapter";

describe("generated HTTP adapter", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("scrubs cycles, aliases, dates, and error details before posting", async () => {
    const fetchMock = vi.fn(async () => new Response(null, { status: 202 }));
    vi.stubGlobal("fetch", fetchMock);

    const shared = { email: "alice@example.com" };
    const context: Record<string, unknown> = { shared, alias: shared };
    context.self = context;
    context.when = new Date("2026-07-11T00:00:00.000Z");
    context.bigint = 123n;

    await reportCanaryError(new Error("failed for alice@example.com"), {
      endpoint: "https://canary.example/",
      apiKey: "sk_live_test-key",
      service: "generated-app",
      context,
    });

    expect(fetchMock).toHaveBeenCalledOnce();
    const [, init] = fetchMock.mock.calls[0] as [string, RequestInit];
    const payload = JSON.parse(String(init.body));
    expect(payload.context.self).toBe("[Circular]");
    expect(payload.context.shared.email).toBe("[REDACTED_EMAIL]");
    expect(payload.context.alias.email).toBe("[REDACTED_EMAIL]");
    expect(payload.context.when).toBe("2026-07-11T00:00:00.000Z");
    expect(payload.context.bigint).toBe("123");
    expect(payload.message).toBe("failed for [REDACTED_EMAIL]");
    expect(payload.stack_trace).not.toContain("alice@example.com");
    expect(init.signal).toBeInstanceOf(AbortSignal);
  });

  it("does not crash when an Error has no constructor", async () => {
    const fetchMock = vi.fn(async () => new Response(null, { status: 202 }));
    vi.stubGlobal("fetch", fetchMock);
    const error = new Error("safe");
    Object.defineProperty(error, "constructor", { value: null });

    await reportCanaryError(error, {
      endpoint: "https://canary.example",
      apiKey: "sk_live_test-key",
      service: "generated-app",
    });

    const [, init] = fetchMock.mock.calls[0] as [string, RequestInit];
    expect(JSON.parse(String(init.body)).error_class).toBe("Error");
  });

  it("does not throw when normalization or transport setup throws", async () => {
    const throwingError = Object.create(Error.prototype) as Error;
    Object.defineProperty(throwingError, "message", {
      get() {
        throw new Error("message unavailable");
      },
    });

    await expect(reportCanaryError(throwingError, {
      endpoint: "https://canary.example",
      apiKey: "sk_live_test-key",
      service: "generated-app",
    })).resolves.toBeUndefined();

    vi.stubGlobal("fetch", undefined);
    await expect(reportCanaryError(new Error("safe"), {
      endpoint: "https://canary.example",
      apiKey: "sk_live_test-key",
      service: "generated-app",
    })).resolves.toBeUndefined();
  });
});
