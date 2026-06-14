import {
  captureException,
  captureMessage,
  type CaptureOptions,
} from "./index";

export interface RequestInfo {
  path: string;
  method: string;
  headers: Record<string, string>;
}

/**
 * Next.js instrumentation hook. Export from `instrumentation.ts`:
 *
 *   export { onRequestError } from "@canary-obs/sdk/nextjs";
 */
export async function onRequestError(
  error: unknown,
  request: RequestInfo,
  opts?: CaptureOptions
): Promise<void> {
  await captureException(error, {
    ...opts,
    context: {
      ...opts?.context,
      path: request.path,
      method: request.method,
    },
  });
}

export interface BrowserObserverOptions extends CaptureOptions {
  captureErrors?: boolean;
  captureUnhandledRejections?: boolean;
}

export function installBrowserErrorObservers(
  opts: BrowserObserverOptions = {}
): () => void {
  if (typeof window === "undefined") {
    return () => undefined;
  }

  const captureErrors = opts.captureErrors ?? true;
  const captureUnhandledRejections = opts.captureUnhandledRejections ?? true;

  const onError = (event: ErrorEvent) => {
    if (!captureErrors) return;
    const error = event.error ?? event.message;
    void captureException(error, {
      ...opts,
      context: {
        ...opts.context,
        source: "browser.error",
        filename: event.filename,
        lineno: event.lineno,
        colno: event.colno,
      },
    });
  };

  const onUnhandledRejection = (event: PromiseRejectionEvent) => {
    if (!captureUnhandledRejections) return;
    void captureException(event.reason, {
      ...opts,
      context: {
        ...opts.context,
        source: "browser.unhandledrejection",
      },
    });
  };

  window.addEventListener("error", onError);
  window.addEventListener("unhandledrejection", onUnhandledRejection);

  return () => {
    window.removeEventListener("error", onError);
    window.removeEventListener("unhandledrejection", onUnhandledRejection);
  };
}

export async function captureNextGlobalError(
  error: Error,
  opts?: CaptureOptions
): Promise<void> {
  await captureException(error, {
    ...opts,
    context: {
      ...opts?.context,
      source: "next.global-error",
    },
  });
}

export async function captureNextErrorBoundary(
  error: Error,
  errorInfo?: { componentStack?: string | null },
  opts?: CaptureOptions
): Promise<void> {
  await captureException(error, {
    ...opts,
    context: {
      ...opts?.context,
      source: "next.error-boundary",
      componentStack: errorInfo?.componentStack ?? undefined,
    },
  });
}

export interface SentryLikeEvent {
  event_id?: string;
  level?: string;
  message?: string;
  exception?: {
    values?: Array<{
      type?: string;
      value?: string;
      stacktrace?: unknown;
    }>;
  };
  tags?: Record<string, unknown>;
  extra?: Record<string, unknown>;
}

export async function captureSentryEvent(
  event: SentryLikeEvent,
  opts?: CaptureOptions
): Promise<void> {
  const firstException = event.exception?.values?.[0];
  const severity = sentryLevelToSeverity(event.level);
  const context = {
    ...opts?.context,
    source: "sentry.bridge",
    sentryEventId: event.event_id,
    sentryTags: event.tags,
    sentryExtra: event.extra,
  };

  if (firstException?.value || firstException?.type) {
    await captureException(
      new Error(firstException.value ?? firstException.type ?? "Sentry event"),
      {
        ...opts,
        severity,
        context: {
          ...context,
          sentryExceptionType: firstException.type,
          sentryStacktrace: firstException.stacktrace,
        },
      }
    );
    return;
  }

  await captureMessage(event.message ?? "Sentry event", {
    ...opts,
    severity,
    context,
  });
}

export function canaryHealthResponse(
  body: Record<string, unknown> = {}
): Response {
  return new Response(JSON.stringify({ status: "ok", ...body }), {
    status: 200,
    headers: { "Content-Type": "application/json" },
  });
}

function sentryLevelToSeverity(
  level: string | undefined
): CaptureOptions["severity"] {
  if (level === "warning" || level === "info") return level;
  return "error";
}
