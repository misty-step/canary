import { captureException, type CaptureOptions } from "./index";

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
