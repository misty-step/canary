import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import {
  canaryHealthResponse,
  captureNextErrorBoundary,
  captureNextGlobalError,
  captureSentryEvent,
  installBrowserErrorObservers,
  onRequestError,
} from "../src/nextjs";
import { initCanary } from "../src/index";

describe("onRequestError", () => {
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
    initCanary({
      endpoint: "https://canary.test",
      apiKey: "sk_test_abc",
      service: "test-svc",
    });
  });

  afterEach(() => {
    globalThis.fetch = originalFetch;
  });

  it("captures server-side errors from Next.js instrumentation", async () => {
    const err = new Error("server crash");

    await onRequestError(err, {
      path: "/api/users",
      method: "GET",
      headers: {},
    });

    expect(fetchSpy).toHaveBeenCalledOnce();
    const body = JSON.parse(fetchSpy.mock.calls[0][1].body);
    expect(body.error_class).toBe("Error");
    expect(body.message).toBe("server crash");
    expect(body.context).toEqual({
      path: "/api/users",
      method: "GET",
    });
  });

  it("includes route info in context", async () => {
    await onRequestError(new Error("oops"), {
      path: "/dashboard",
      method: "POST",
      headers: { "x-request-id": "req-123" },
    });

    const body = JSON.parse(fetchSpy.mock.calls[0][1].body);
    expect(body.context.path).toBe("/dashboard");
    expect(body.context.method).toBe("POST");
  });

  it("merges existing context with request info", async () => {
    await onRequestError(
      new Error("oops"),
      {
        path: "/dashboard",
        method: "POST",
        headers: {},
      },
      { context: { requestId: "req-123" } }
    );

    const body = JSON.parse(fetchSpy.mock.calls[0][1].body);
    expect(body.context).toEqual({
      requestId: "req-123",
      path: "/dashboard",
      method: "POST",
    });
  });

  it("captures Next.js global errors with a source marker", async () => {
    await captureNextGlobalError(new Error("global crash"));

    const body = JSON.parse(fetchSpy.mock.calls[0][1].body);
    expect(body.context.source).toBe("next.global-error");
    expect(body.message).toBe("global crash");
  });

  it("captures error boundary component stacks", async () => {
    await captureNextErrorBoundary(
      new Error("boundary crash"),
      { componentStack: "at Widget" },
      { context: { route: "/dashboard" } }
    );

    const body = JSON.parse(fetchSpy.mock.calls[0][1].body);
    expect(body.context).toEqual({
      route: "/dashboard",
      source: "next.error-boundary",
      componentStack: "at Widget",
    });
  });

  it("bridges Sentry exception events without depending on Sentry", async () => {
    await captureSentryEvent({
      event_id: "evt-123",
      level: "warning",
      exception: { values: [{ type: "TypeError", value: "bad call" }] },
      tags: { runtime: "edge" },
    });

    const body = JSON.parse(fetchSpy.mock.calls[0][1].body);
    expect(body.severity).toBe("warning");
    expect(body.message).toBe("bad call");
    expect(body.context.source).toBe("sentry.bridge");
    expect(body.context.sentryEventId).toBe("evt-123");
    expect(body.context.sentryExceptionType).toBe("TypeError");
  });

  it("bridges Sentry message events as Canary messages", async () => {
    await captureSentryEvent({ event_id: "evt-456", level: "info", message: "soft warning" });

    const body = JSON.parse(fetchSpy.mock.calls[0][1].body);
    expect(body.error_class).toBe("Message");
    expect(body.severity).toBe("info");
    expect(body.message).toBe("soft warning");
  });

  it("bridges sparse Sentry events with default message and severity", async () => {
    await captureSentryEvent({ event_id: "evt-789" });

    const body = JSON.parse(fetchSpy.mock.calls[0][1].body);
    expect(body.error_class).toBe("Message");
    expect(body.severity).toBe("error");
    expect(body.message).toBe("Sentry event");
  });

  it("returns a no-op browser observer cleanup outside browsers", () => {
    const cleanup = installBrowserErrorObservers();

    expect(cleanup()).toBeUndefined();
    expect(fetchSpy).not.toHaveBeenCalled();
  });

  it("installs removable browser error observers", async () => {
    const listeners = new Map<string, EventListener>();
    const originalWindow = globalThis.window;
    Object.defineProperty(globalThis, "window", {
      value: {
        addEventListener: vi.fn((name: string, listener: EventListener) => {
          listeners.set(name, listener);
        }),
        removeEventListener: vi.fn((name: string) => {
          listeners.delete(name);
        }),
      },
      configurable: true,
    });

    const uninstall = installBrowserErrorObservers();
    listeners.get("error")!(
      {
        error: new Error("browser crash"),
        filename: "app.js",
        lineno: 7,
        colno: 11,
      } as ErrorEvent
    );
    await Promise.resolve();
    uninstall();

    const body = JSON.parse(fetchSpy.mock.calls[0][1].body);
    expect(body.message).toBe("browser crash");
    expect(body.context.source).toBe("browser.error");
    expect(body.context.filename).toBe("app.js");
    expect(listeners.size).toBe(0);
    Object.defineProperty(globalThis, "window", {
      value: originalWindow,
      configurable: true,
    });
  });

  it("honors disabled browser observer channels", async () => {
    const listeners = new Map<string, EventListener>();
    const originalWindow = globalThis.window;
    Object.defineProperty(globalThis, "window", {
      value: {
        addEventListener: vi.fn((name: string, listener: EventListener) => {
          listeners.set(name, listener);
        }),
        removeEventListener: vi.fn(),
      },
      configurable: true,
    });

    installBrowserErrorObservers({
      captureErrors: false,
      captureUnhandledRejections: false,
    });
    listeners.get("error")!({ message: "ignored" } as ErrorEvent);
    listeners.get("unhandledrejection")!({ reason: new Error("ignored") } as PromiseRejectionEvent);
    await Promise.resolve();

    expect(fetchSpy).not.toHaveBeenCalled();
    Object.defineProperty(globalThis, "window", {
      value: originalWindow,
      configurable: true,
    });
  });

  it("creates a standard health response", async () => {
    const response = canaryHealthResponse({ build: "abc" });

    expect(response.status).toBe(200);
    expect(response.headers.get("content-type")).toBe("application/json");
    await expect(response.json()).resolves.toEqual({ status: "ok", build: "abc" });
  });
});
