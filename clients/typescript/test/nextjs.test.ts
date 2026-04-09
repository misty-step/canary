import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { onRequestError } from "../src/nextjs";
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
});
