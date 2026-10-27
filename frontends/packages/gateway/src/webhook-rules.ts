// What a `WebhookEndpoint` may hold, written once for every place in the console
// that checks it (the Webhooks screen's forms and the tRPC router behind them).
// The gateway enforces both again on write (`schemas/vsms.cstack`'s `@regex` on
// `url` and `audience`, a CHECK and a unique index on `audience`), so these are
// the early, friendly half of the same rules, never the only one.

/**
 * The audience the gateway's own routes answer to. A receiver that used it for
 * its own endpoint would give up the protection against a request being
 * reflected back at the gateway, so no endpoint may carry it.
 */
export const GATEWAY_AUDIENCE = "vsms";

/** The message for a refused audience; one sentence for the form and the router. */
export const GATEWAY_AUDIENCE_MESSAGE = `"${GATEWAY_AUDIENCE}" is the gateway's own name: choose one that is yours (your origin works)`;

/**
 * The route a delivery to `url` binds: its path, or `null` when the URL cannot
 * carry a sealed delivery. The same rule as `sms_webhook::delivery_route`
 * (http(s), no credentials, no query, no fragment, a path with no `{`, `}` or
 * `%`), which is what the worker applies at delivery time: a URL this refuses
 * would only fail there, burning attempts and tripping the endpoint's breaker.
 * `sms-webhook`'s `tests/fixtures/delivery_route_cases.json` holds this and the
 * Rust function to the same answers.
 *
 * It differs from `new URL()` in one place on purpose: `?` and `#` are looked
 * for in the text, because the parsed `search` and `hash` of `https://h/a?` and
 * `https://h/a#` are empty (the URL API drops a bare marker) while the Rust
 * parser still sees a query and a fragment. (The gateway is stricter on one
 * more point and says so when the form is submitted: a host is stored in its
 * ASCII, punycode, form.)
 */
export function receiverRoute(url: string): string | null {
  if (url.includes("?") || url.includes("#")) return null;
  let parsed: URL;
  try {
    parsed = new URL(url);
  } catch {
    return null;
  }
  if (parsed.protocol !== "http:" && parsed.protocol !== "https:") return null;
  if (parsed.username !== "" || parsed.password !== "") return null;
  if (/[{}%]/.test(parsed.pathname)) return null;
  return parsed.pathname;
}

/** Why `url` cannot be a webhook endpoint's, or `null` when it can. */
export function urlRefusal(url: string): string | null {
  return receiverRoute(url) === null
    ? "Must be an http(s) URL with no credentials, query, fragment or {}% in its path: a delivery binds the path as its route"
    : null;
}
