// #59: "thread ETag / If-Match through every edit ... in the data layer
// once." This is that layer — a generic `GET`-with-`ETag` /
// `PATCH`-with-`If-Match` pair, so a future edit screen (#52–#58) gets 412
// handling by wiring a model-specific wrapper on top of these two
// functions, the same way `messages.ts`'s `getJson` is the one place
// `listMessages`/`getMessageById` both go through — never by remembering to
// ask for it per screen.
//
// #54 (the first real consumer of that promise) adds `postJson`/
// `deleteResource` alongside — `Route`'s own create/delete, neither of
// which touches an `ETag` at all (a new row has nothing to send `If-Match`
// against; `DELETE` needs none per the generated handler itself, see
// `deleteResource`'s own doc). Kept in this file rather than a new one:
// they're the same `gatewayFetch` shape as
// `fetchWithEtag`/`updateWithIfMatch`, just without the header dance.
//
// # What proves it
//
// `backends/crates/sms-api/tests/if_match_live_postgres.rs` (Rust side of #59)
// is the end-to-end proof: a real Postgres, the real generated CAS SQL two
// operators racing a `Provider` update actually executes, a real
// `CratestackError::PreconditionFailed` mapping to a real HTTP 412. These two
// functions are tested (`rest.test.ts`) for what this layer owns: real header
// parsing, real `If-Match` attachment, real error-shape mapping.
//
// # Who the call is made as
//
// Nobody in this package holds a credential. Every function here writes the
// request it wants as an ordinary one and hands it to `gatewayFetch`
// (`./upstream.ts`); in the console that is the sealing fetch
// (`./sealing-fetch.ts`), which signs it with the paired person's own browser
// key, so the gateway authenticates the signed-in person (`SignedAuth`,
// `backends/crates/sms-api/src/auth.rs`) and their role's policies decide. A 401
// is the gateway's unsigned refusal of a key it does not accept, and is a
// denial, not a prompt to retry.

import { mapGatewayError } from "./errors";
import { parseGatewayJson } from "./json";
import { GATEWAY_ORIGIN, gatewayFetch } from "./upstream";

type Fetcher = typeof gatewayFetch;

export interface WithEtag<T> {
  data: T;
  /**
   * The row's optimistic-lock version, read verbatim off the response's
   * `ETag` header — `cratestack-macros`'s generated `GET`/detail handler
   * (`prep/etag.rs`, read directly) stamps this from the row's own
   * `version` field on every 2xx for a `@version`'d model: a strong
   * validator of the form `"<integer>"`, quotes included. `undefined` when
   * the response carried no `ETag` at all — a model with no `@version`
   * field, or a non-2xx this function already turned into a thrown
   * `GatewayError` before returning, never reaches the `undefined` case in
   * practice for the first reason alone, but a wrapper for a model this
   * package doesn't yet know is versioned should still handle it rather
   * than assume.
   */
  etag: string | undefined;
}

/**
 * Normalise an `If-Match` validator to the strong-ETag form the server
 * actually parses.
 *
 * **The quotes are mandatory, and their absence does not fail the way you
 * would expect.** `cratestack-axum`'s `parse_if_match_version`
 * (`src/headers/etag.rs`, read directly) does:
 *
 * ```rust
 * raw.strip_prefix('"').and_then(|s| s.strip_suffix('"'))
 *    .ok_or_else(|| CratestackError::BadRequest(
 *        "If-Match must be a strong ETag of the form \"<integer>\""))?
 * ```
 *
 * So a bare `3` does not merely fail the precondition and return 412 — it
 * fails to *parse*, and the request 400s before any version comparison
 * happens. That is a materially different failure: a 412 means "someone
 * else edited this, reload", which the UI handles; a 400 means the write
 * never had a chance and reads as a server fault.
 *
 * Normalising here rather than at each call site is deliberate. Screens
 * are client components and cannot import this `server-only` package, so
 * a shared client-side helper is not available to them — which is exactly
 * how #220's client-retire path came to send `String(client.version)` and
 * would have 400'd on every attempt, while its sibling screen constructed
 * the quoted form by hand and worked. This seam is the one place that
 * knows the wire format; making it authoritative means a caller cannot
 * get it wrong by having the integer rather than the header.
 *
 * A validator that already carries quotes is passed through untouched, so
 * an `etag` captured verbatim from a `GET` — always preferable, since it
 * is the row's own validator rather than a reconstruction — is unaffected.
 */
function normaliseIfMatch(etag: string): string {
  const trimmed = etag.trim();
  if (trimmed.startsWith('"') && trimmed.endsWith('"')) {
    return trimmed;
  }
  return `"${trimmed}"`;
}

function restUrl(path: string): string {
  return new URL(path, GATEWAY_ORIGIN).toString();
}

/**
 * `GET <path>` — same shape as `client.ts`'s `callProcedure` and `messages.ts`'s
 * `getJson`, duplicated rather than shared for the same reason those two
 * already give: each of these three is a small, independently-replaceable
 * half of the same temporary seam (T3 replaces all of them at once).
 * `fetcher` is injectable so `rest.test.ts` can stand in a fake upstream
 * without needing a gateway or a signing key — the same
 * dependency-injection shape `message-stream.ts`'s own `fetchWindow`
 * already uses for the identical reason.
 */
export async function fetchWithEtag<T>(
  path: string,
  routeLabel: string,
  fetcher: Fetcher = gatewayFetch,
): Promise<WithEtag<T> | null> {
  const url = restUrl(path);

  const response = await fetcher(url, {
    method: "GET",
    headers: {
      accept: "application/json",
    },
  });

  if (response.status === 404) return null;

  const parsed = await parseGatewayJson(response);
  if (!response.ok) {
    throw mapGatewayError(response.status, parsed, routeLabel);
  }

  return { data: parsed as T, etag: response.headers.get("etag") ?? undefined };
}

/**
 * `PATCH <path>` with `If-Match: <etag>` and a JSON body. A stale `etag` — another operator's edit landed on
 * this row first — surfaces as the same `GatewayError` `mapGatewayError`
 * already builds for a 412 (`trpcCode: "CONFLICT"`,
 * `gatewayCode: "PRECONDITION_FAILED"`, both set before this function
 * existed — see `errors.ts`'s own module doc, written anticipating exactly
 * this route). `isStaleWriteError` (`errors.ts`) is the one thing a screen
 * needs to check to show "someone else changed this, reload" instead of a
 * generic error toast — everything else about the 412 shape is already
 * handled by `mapGatewayError`, not duplicated here.
 */
export async function updateWithIfMatch<T>(
  path: string,
  body: unknown,
  etag: string,
  routeLabel: string,
  fetcher: Fetcher = gatewayFetch,
): Promise<WithEtag<T>> {
  const url = restUrl(path);
  const payload = JSON.stringify(body);

  const response = await fetcher(url, {
    method: "PATCH",
    headers: {
      "content-type": "application/json",
      accept: "application/json",
      "if-match": normaliseIfMatch(etag),
    },
    body: payload,
  });

  const parsed = await parseGatewayJson(response);
  if (!response.ok) {
    throw mapGatewayError(response.status, parsed, routeLabel);
  }

  return { data: parsed as T, etag: response.headers.get("etag") ?? undefined };
}

/**
 * `POST <path>` with a JSON body — #54's `Route` create, the one write this
 * ticket's screens need that isn't an `If-Match`-guarded edit (a brand-new
 * row has no prior `ETag` to send). `routeLabel` is only for
 * `mapGatewayError`'s own log line, as everywhere else.
 */
export async function postJson<T>(
  path: string,
  body: unknown,
  routeLabel: string,
  fetcher: Fetcher = gatewayFetch,
): Promise<T> {
  const url = restUrl(path);
  const payload = JSON.stringify(body);

  const response = await fetcher(url, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      accept: "application/json",
    },
    body: payload,
  });

  const parsed = await parseGatewayJson(response);
  if (!response.ok) {
    throw mapGatewayError(response.status, parsed, routeLabel);
  }

  return parsed as T;
}

/**
 * `DELETE <path>` — #54's `Route` delete, and every other model-delete
 * screen built on top of it since.
 *
 * **Fixed for the cratestack 0.7.16 bump: this function now sends an
 * `ETag` as `If-Match`.** The doc here used to say the generated delete
 * handler needed no `If-Match` at all
 * (`cratestack-macros-0.7.10/src/axum/model/prep/etag.rs`'s `EtagTokens`
 * only wired `update_if_match_*`/`get_etag_*`, nothing for delete — true
 * at the time, read directly rather than assumed). cratestack 0.7.13
 * (cratestack#519, `cratestack-macros`'s `prep/etag.rs` gaining
 * `delete_if_match_decl`/`delete_if_match_apply`) closed that asymmetry:
 * `DELETE` on an `@version` model now requires `If-Match` and returns
 * `412` on a stale or missing value, exactly like `PATCH` already did.
 * Every caller of this function (`deleteRoute`, `deleteWebhookEndpoint`,
 * `deleteApp`… — grep `deleteResource(`) targets a `@version`'d model
 * (`Route`/`WebhookEndpoint`/`App`/`User`/`Role`, per #59), so without
 * this fix every one of those delete buttons would genuinely 412 against
 * a real 0.7.16 gateway. Verified live against a real gateway
 * (`just demo`), not just reasoned about: a real delete now succeeds, and
 * a deliberately stale `If-Match` still produces a real `412` — see
 * `AGENTS.md`'s 0.7.16 bump section for both terminal transcripts.
 *
 * **The optional `etag` parameter is the better long-term shape this
 * function's own doc used to name as future work — it's built now, not
 * deferred again.** A caller that already holds the row's current
 * `version`/`ETag` — which every screen does, since it necessarily
 * rendered the row to offer a delete button in the first place — passes
 * it straight through: `deleteResource` sends it verbatim as `If-Match`
 * and skips the internal `GET` entirely, so a known-version delete is a
 * single round trip with no TOCTOU window at all. `etag` is normalised the
 * same way [`updateWithIfMatch`]'s own `etag` parameter is (bare or
 * quoted, either works — see [`normaliseIfMatch`]), so a caller can pass a
 * captured `WithEtag.etag` verbatim or a plain `String(row.version)`, the
 * same two shapes `updateRoute`/`updateWebhookEndpoint`/etc. already
 * accept.
 *
 * **When `etag` is omitted, this falls back to the original shape: a
 * `GET` (via `fetchWithEtag`) to capture the row's current `ETag`, then
 * the `DELETE` carrying it as `If-Match`.** This keeps every caller that
 * doesn't have a version handy working exactly as before — deliberately
 * kept as a fallback rather than requiring every call site to be updated
 * in lockstep, since not every future model-delete screen will
 * necessarily hold the row's version in state at the point it calls this.
 *
 * **Be honest about what the fallback still costs.** There is a real
 * TOCTOU window between the `GET` and the `DELETE` in the fallback path:
 * if the row is edited in between (another operator's concurrent
 * `PATCH`), the captured `ETag` is stale and the `DELETE` gets a real
 * `412` — that is `If-Match` doing exactly its job, not a bug, but it
 * does mean the fallback can fail a delete that would have succeeded a
 * moment earlier. The known-version path above avoids this entirely; the
 * fallback exists only for a caller that genuinely has no version to
 * supply.
 *
 * A `404` on the fallback `GET` means the row is already gone — nothing
 * to send `If-Match` against — so the `DELETE` below still runs with no
 * `If-Match` header at all, and its own response is what decides the
 * outcome, unchanged from this function's pre-existing behaviour for a
 * row that doesn't exist. A model with no `@version` field (`OptOut`'s
 * own hand-rolled delete, not routed through this function, is the one
 * example today) never has an `ETag` to capture either, so this adds no
 * behaviour there.
 */
export async function deleteResource(
  path: string,
  routeLabel: string,
  etag?: string,
  fetcher: Fetcher = gatewayFetch,
): Promise<void> {
  const url = restUrl(path);

  let ifMatch: string | undefined;
  if (etag !== undefined) {
    ifMatch = normaliseIfMatch(etag);
  } else {
    const existing = await fetchWithEtag<unknown>(path, routeLabel, fetcher);
    if (existing?.etag !== undefined) {
      ifMatch = normaliseIfMatch(existing.etag);
    }
  }

  const headers: Record<string, string> = {
    accept: "application/json",
  };
  if (ifMatch !== undefined) {
    headers["if-match"] = ifMatch;
  }
  const response = await fetcher(url, {
    method: "DELETE",
    headers,
  });

  if (!response.ok) {
    const parsed = await parseGatewayJson(response);
    throw mapGatewayError(response.status, parsed, routeLabel);
  }
}
