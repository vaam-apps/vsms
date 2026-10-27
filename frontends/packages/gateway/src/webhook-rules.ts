// What a `WebhookEndpoint` may hold, written once for every place in the console
// that checks it (the Webhooks screen's forms and the tRPC router behind them).
// The gateway enforces both again on write (a CHECK on `url` for a `sealed`
// endpoint alone, `@regex` and a CHECK on `audience`, a unique index on it), so
// these are the early, friendly half of the same rules, never the only one.
// The URL rules are a `sealed` endpoint's: an `hmac` delivery binds no route and
// keeps whatever URL it was registered with.

/**
 * The audience the gateway's own routes answer to. A receiver that used it for
 * its own endpoint would give up the protection against a request being
 * reflected back at the gateway, so no endpoint may carry it.
 */
export const GATEWAY_AUDIENCE = "vsms";

/** The message for a refused audience; one sentence for the form and the router. */
export const GATEWAY_AUDIENCE_MESSAGE = `"${GATEWAY_AUDIENCE}" is the gateway's own name: choose one that is yours (your origin works)`;

/**
 * The URLs the database lets a `sealed` endpoint hold, written once more here.
 * It is `sms_webhook::SEALABLE_URL_PATTERN` character for character: migration
 * `0010_sealed_webhooks` carries it as the CHECK
 * `webhook_endpoints_url_sealable_for_sealed_check`, and the Rust constant is
 * what that migration, the schema and `sms-api`'s tests are held to.
 * `webhook-rules.test.ts` reads the Rust constant out of
 * `backends/crates/sms-webhook/src/lib.rs` and fails if this literal differs, and
 * runs `sms-webhook`'s `tests/fixtures/sealable_url_cases.json` (which the Rust
 * test reads too) through it, so it cannot drift from either. Never edit it here
 * alone.
 *
 * It matches the text as typed, and is narrower than `new URL()` on purpose: an
 * internationalised host is written in punycode (`bücher.example` is refused,
 * `xn--bcher-kva.example` is not), a host has no underscore, and a port is 1 to
 * 65535. It is also wider than URL parsing in places (an IPv6 literal of `[:]`,
 * `1.2.3.999`, `x.0`), which is why [`urlCanBeSealed`] asks both.
 */
export const SEALABLE_URL_PATTERN = String.raw`^[Hh][Tt][Tt][Pp][Ss]?://(?:[A-Za-z0-9](?:[A-Za-z0-9.-]*[A-Za-z0-9])?|\[[0-9A-Fa-f:.]+\])(?::(?:6553[0-5]|655[0-2][0-9]|65[0-4][0-9]{2}|6[0-4][0-9]{3}|[1-5][0-9]{4}|[1-9][0-9]{0,3}))?(?:/[A-Za-z0-9._~!$&'()*+,;=:@/-]*)?$`;

const SEALABLE_URL = new RegExp(SEALABLE_URL_PATTERN);

/** Whether `url`, as typed, is one the database lets a `sealed` endpoint hold. */
export function urlMatchesSealablePattern(url: string): boolean {
  return SEALABLE_URL.test(url);
}

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
 * parser still sees a query and a fragment. It is the route rule alone: the
 * text the database accepts is [`SEALABLE_URL_PATTERN`], and [`urlCanBeSealed`]
 * asks both.
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

/**
 * Whether a `sealed` endpoint may be registered at `url`: the database's pattern
 * ([`SEALABLE_URL_PATTERN`]) **and** a route the worker can bind
 * ([`receiverRoute`]). Neither implies the other: the pattern is stricter about
 * the text (ASCII host, port range) and the route about what the URL parses to
 * (`[:]`, `1.2.3.999` and `x.0` match the pattern and do not parse).
 */
export function urlCanBeSealed(url: string): boolean {
  return urlMatchesSealablePattern(url) && receiverRoute(url) !== null;
}

/** Why `url` cannot be a `sealed` webhook endpoint's, or `null` when it can. */
export function urlRefusal(url: string): string | null {
  return urlCanBeSealed(url)
    ? null
    : "Must be an http(s) URL with an ASCII host (write an internationalised name in punycode), a port from 1 to 65535 and no credentials, query, fragment or {}% in its path: a delivery binds the path as its route";
}
