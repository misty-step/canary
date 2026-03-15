import { createClient, type CanaryClient, type CanaryResponse } from "./client";
import { scrub, scrubObject, type ScrubRule } from "./scrub";

export type { CanaryResponse, ScrubRule };

export interface InitOptions {
  endpoint: string;
  apiKey: string;
  service: string;
  environment?: string;
  scrubPii?: boolean;
  scrubRules?: ScrubRule[];
}

export interface CaptureOptions {
  severity?: "error" | "warning" | "info";
  context?: Record<string, unknown>;
  fingerprint?: string[];
}

let client: CanaryClient | null = null;
let scrubPii = false;
let scrubRules: ScrubRule[] = [];

export function initCanary(opts: InitOptions): void {
  client = createClient(opts);
  scrubPii = opts.scrubPii ?? false;
  scrubRules = opts.scrubRules ?? [];
}

export async function captureException(
  error: unknown,
  opts: CaptureOptions = {}
): Promise<CanaryResponse | null> {
  if (!client) return null;

  const { errorClass, message, stackTrace } = normalizeError(error);

  return client.send({
    error_class: errorClass,
    message: scrubPii ? scrub(message, scrubRules)! : message,
    severity: opts.severity ?? "error",
    stack_trace: scrubPii ? scrub(stackTrace, scrubRules) : stackTrace,
    context: scrubPii ? scrubObject(opts.context, scrubRules) : opts.context,
    fingerprint: opts.fingerprint,
  });
}

export async function captureMessage(
  message: string,
  opts: CaptureOptions = {}
): Promise<CanaryResponse | null> {
  if (!client) return null;

  return client.send({
    error_class: "Message",
    message: scrubPii ? scrub(message, scrubRules)! : message,
    severity: opts.severity ?? "info",
    context: scrubPii ? scrubObject(opts.context, scrubRules) : opts.context,
    fingerprint: opts.fingerprint,
  });
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
