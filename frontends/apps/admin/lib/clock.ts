// Whether this computer's clock is close enough to the gateway's to sign.
//
// Every sealed request carries the time it was signed, and the gateway refuses
// one more than five minutes off its own clock (the envelope's skew), as it
// refuses a pairing QR for the same reason. A laptop whose clock is wrong by
// more than that cannot use the console at all, and the failure looks like "not
// paired" (the gateway's refusal is the same unsigned 401), so the pairing page
// measures the difference and says so instead.
//
// The reference is the `Date` header of the console's own server, which sits
// beside the gateway and is not this person's laptop.

/** How far off is worth a warning: well inside the gateway's five minutes, so a
 *  person is told before their requests start failing. */
export const CLOCK_SKEW_WARN_SECONDS = 120;

/** This computer's clock minus the server's, in seconds, or `undefined` when
 *  the header is missing or unreadable. `now` is read after the answer arrived,
 *  so the figure includes the request's latency: seconds, not milliseconds,
 *  are what it is good for. */
export function clockSkewSeconds(dateHeader: string | null, now: number): number | undefined {
  if (dateHeader === null) return undefined;
  const server = Date.parse(dateHeader);
  if (Number.isNaN(server)) return undefined;
  return Math.round((now - server) / 1000);
}

/** Whether `skew` is large enough to warn about. */
export function isSkewed(skew: number | undefined): boolean {
  return skew !== undefined && Math.abs(skew) > CLOCK_SKEW_WARN_SECONDS;
}

/** Measures this computer's clock against the console's server, or `undefined`
 *  when the server cannot be asked. The one reader of the figure: the pairing
 *  page's warning and the key's status (`./key-lifecycle.ts`) must agree on it. */
export async function measureClockSkew(
  options: { fetch?: typeof fetch; now?: () => number } = {},
): Promise<number | undefined> {
  const get = options.fetch ?? ((input, init) => globalThis.fetch(input, init));
  const now = options.now ?? Date.now;
  try {
    const response = await get("/api/health", { cache: "no-store", credentials: "omit" });
    return clockSkewSeconds(response.headers.get("date"), now());
  } catch {
    return undefined;
  }
}
