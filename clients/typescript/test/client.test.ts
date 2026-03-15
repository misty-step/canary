import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { createClient, type CanaryClient } from "../src/client";

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

  it("drops oldest when queue exceeds max", async () => {
    // Never resolve — simulate slow network
    let resolvers: Array<(v: Response) => void> = [];
    fetchSpy.mockImplementation(
      () =>
        new Promise<Response>((resolve) => {
          resolvers.push(resolve);
        })
    );

    const client = createClient({ ...opts, maxQueue: 3 });

    // Fire 4 sends without awaiting
    client.send({ error_class: "E1", message: "1", severity: "error" });
    client.send({ error_class: "E2", message: "2", severity: "error" });
    client.send({ error_class: "E3", message: "3", severity: "error" });
    client.send({ error_class: "E4", message: "4", severity: "error" });

    // Queue should have capped at 3 pending
    expect(client.pending).toBeLessThanOrEqual(3);
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
});
