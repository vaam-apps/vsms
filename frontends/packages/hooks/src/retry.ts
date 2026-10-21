// The retry policy every console query runs under.
//
// TanStack retries a failed query three times with exponential backoff
// (about 7 seconds in all) before it reports the error. For a failure the
// server will answer the same way every time (a 403 for a missing
// permission, a 404 for a message that does not exist or is not yours, a
// 400 for a bad filter) that is seven seconds of skeleton before the screen
// says anything, measured on the message detail page with an unknown id.
// A 4xx is the server's answer, not a fault, so it is shown at once; a
// network failure or a 5xx is still retried, up to three times.

const MAX_RETRIES = 3;

function httpStatusOf(error: unknown): number | undefined {
  if (typeof error !== "object" || error === null || !("data" in error)) return undefined;
  const data = (error as { data?: unknown }).data;
  if (typeof data !== "object" || data === null || !("httpStatus" in data)) return undefined;
  const status = (data as { httpStatus?: unknown }).httpStatus;
  return typeof status === "number" ? status : undefined;
}

export function shouldRetryQuery(failureCount: number, error: unknown): boolean {
  const status = httpStatusOf(error);
  if (status !== undefined && status >= 400 && status < 500) return false;
  return failureCount < MAX_RETRIES;
}
