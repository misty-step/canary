/**
 * Canary TypeScript client — thin HTTP wrapper for error reporting.
 *
 * Usage:
 *   import { Canary } from "./canary";
 *
 *   const canary = new Canary({
 *     endpoint: "https://canary-obs.fly.dev",
 *     apiKey: process.env.CANARY_API_KEY!,
 *     service: "volume",
 *   });
 *
 *   // In catch blocks:
 *   canary.capture(error, { userId: "abc", endpoint: "/api/sessions" });
 */

export interface CanaryOptions {
  endpoint: string;
  apiKey: string;
  service: string;
  environment?: string;
  enabled?: boolean;
}

export interface CaptureOptions {
  severity?: "error" | "warning" | "info";
  context?: Record<string, unknown>;
  fingerprint?: string[];
}

export interface CanaryResponse {
  id: string;
  group_hash: string;
  is_new_class: boolean;
}

export class Canary {
  private endpoint: string;
  private apiKey: string;
  private service: string;
  private environment: string;
  private enabled: boolean;

  constructor(options: CanaryOptions) {
    this.endpoint = options.endpoint.replace(/\/$/, "");
    this.apiKey = options.apiKey;
    this.service = options.service;
    this.environment = options.environment ?? "production";
    this.enabled = options.enabled ?? true;
  }

  async capture(
    error: unknown,
    options: CaptureOptions = {}
  ): Promise<CanaryResponse | null> {
    if (!this.enabled) return null;

    const { errorClass, message, stackTrace } = normalizeError(error);

    const body = {
      service: this.service,
      error_class: errorClass,
      message,
      stack_trace: stackTrace,
      severity: options.severity ?? "error",
      environment: this.environment,
      context: options.context ? sanitizeContext(options.context) : undefined,
      fingerprint: options.fingerprint,
    };

    try {
      const response = await fetch(`${this.endpoint}/api/v1/errors`, {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          Authorization: `Bearer ${this.apiKey}`,
        },
        body: JSON.stringify(body),
        signal: AbortSignal.timeout(5_000),
      });

      if (!response.ok) return null;
      return (await response.json()) as CanaryResponse;
    } catch {
      // Never throw from error reporting — swallow silently
      return null;
    }
  }
}

function normalizeError(error: unknown): {
  errorClass: string;
  message: string;
  stackTrace: string | undefined;
} {
  if (error instanceof Error) {
    return {
      errorClass: error.constructor.name || "Error",
      message: error.message,
      stackTrace: error.stack,
    };
  }

  if (typeof error === "string") {
    return { errorClass: "StringError", message: error, stackTrace: undefined };
  }

  return {
    errorClass: "UnknownError",
    message: String(error),
    stackTrace: undefined,
  };
}

function sanitizeContext(
  context: Record<string, unknown>
): Record<string, unknown> {
  const json = JSON.stringify(context);
  if (json.length > 8192) return { _truncated: true, _size: json.length };
  return context;
}
