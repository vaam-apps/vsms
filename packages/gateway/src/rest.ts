import "server-only";

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
// they're the same `undiciFetch`-with-a-Bearer-token-and-401-retry shape as
// `fetchWithEtag`/`updateWithIfMatch`, just without the header dance.
//
// # What "the smallest real one" proved, and what it didn't
//
// `crates/sms-api/tests/if_match_live_postgres.rs` (Rust side of #59) is
// the genuine end-to-end proof: a real Postgres, the real generated CAS SQL
// two operators racing a `Provider` update actually executes, a real
// `CoolError::PreconditionFailed` mapping to a real HTTP 412 — verified
// against `cratestack-core`'s own `error.rs`, not assumed. That test's own
// module doc explains why it stops at the delegate layer rather than going
// over real HTTP through this package: `GatewayAuth::authenticate`
// (`crates/sms-api/src/auth.rs`) hardcodes `role: "app"` for *every* real
// token this deployment ever mints — not read from any claim, a stronger
// finding than `rbac_layer2_live_postgres.rs`'s own "no live success case"
// note — and every one of #59's ten newly `@version`'d models needs a
// human role (`owner`/`admin`/`operator`/`developer`) to write. This
// package's own service account (`token.ts`, `SMS_CONSOLE_CLIENT_ID`) is
// exactly such a real token: `kind: "app"`, never anything else. So the
// exact route these two functions exist to call — `PATCH /providers/{id}`,
// `PATCH /routes/{id}`, etc. — is **not reachable by this console's own
// credential today**, for the identical structural reason, not a gap this
// ticket introduced or can close. That's real infrastructure work (a
// human-login `AuthProvider`), tracked separately, same scope cut #24/#25
// already recorded for Layer 2. These two functions are built and tested
// (`rest.test.ts`) against that reality: real header parsing, real
// `If-Match` attachment, real error-shape mapping — everything this layer
// owns — with a fake upstream standing in for sms-api, not a live one,
// because no real one is callable yet for any model this ticket touches.
//
// **Correction, #54: the "`GatewayAuth::authenticate` hardcodes `role:
// "app"` for every real token" line above is no longer true — #194 (human
// login) landed after this paragraph was written, and `GatewayAuth` now
// genuinely resolves a real `hasRole(...)`-meaningful context for a human
// `authorization_code` token.** What the paragraph's own conclusion still
// got right, for a narrower reason that #54 itself was correct about:
// `token.ts`'s `SMS_CONSOLE_CLIENT_ID` credential is a *separate*
// `client_credentials` `AppClient`, untouched by #194, and at the time #54
// landed every function in this file (and every screen that calls one)
// still authenticated exclusively through it.
//
// **Correction, #211: that is no longer true either.** `getAccessToken`
// above is now `resolveUpstreamAccessToken` (`./request-credential.ts`),
// which forwards the signed-in operator's own session token
// (`admin/lib/oidc.ts::Session.accessToken`) when this call is running
// inside a real admin-console request — see that module's own doc for the
// mechanism (`AsyncLocalStorage`, set once at the tRPC route handler) and
// for why a 401 here is a genuine denial rather than a retry-with-a-fresh-
// token case the way it is for the machine credential. `PATCH
// /providers/{id}` etc. are reachable today by a real, signed-in
// `owner`/`admin`/`operator` — proven live in #211's own PR description,
// not merely reasoned about.

import { env } from "@vsms/env";
import { fetch as undiciFetch } from "undici";
import { gatewayAgent } from "./dispatcher";
import { mapGatewayError } from "./errors";
import { parseGatewayJson } from "./json";
import { invalidateUpstreamAccessToken, resolveUpstreamAccessToken } from "./request-credential";

type UndiciResponse = Awaited<ReturnType<typeof undiciFetch>>;
type Fetcher = typeof undiciFetch;

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
 *    .ok_or_else(|| CoolError::BadRequest(
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
  return new URL(path, env.SMS_API_URL).toString();
}

/**
 * `GET <path>` with a Bearer token, retrying once on an unexpected 401 —
 * same shape as `client.ts`'s `callProcedure` and `messages.ts`'s
 * `getJson`, duplicated rather than shared for the same reason those two
 * already give: each of these three is a small, independently-replaceable
 * half of the same temporary seam (T3 replaces all of them at once).
 * `fetcher` is injectable so `rest.test.ts` can stand in a fake upstream
 * without needing `SMS_API_URL`/token machinery configured — the same
 * dependency-injection shape `message-stream.ts`'s own `fetchWindow`
 * already uses for the identical reason.
 */
export async function fetchWithEtag<T>(
  path: string,
  routeLabel: string,
  fetcher: Fetcher = undiciFetch,
): Promise<WithEtag<T> | null> {
  const url = restUrl(path);

  const attempt = async (): Promise<UndiciResponse> => {
    const token = await resolveUpstreamAccessToken();
    return fetcher(url, {
      method: "GET",
      headers: {
        accept: "application/json",
        authorization: `Bearer ${token}`,
      },
      dispatcher: gatewayAgent(),
    });
  };

  let response = await attempt();
  if (response.status === 401) {
    invalidateUpstreamAccessToken();
    response = await attempt();
  }

  if (response.status === 404) return null;

  const parsed = await parseGatewayJson(response);
  if (!response.ok) {
    throw mapGatewayError(response.status, parsed, routeLabel);
  }

  return { data: parsed as T, etag: response.headers.get("etag") ?? undefined };
}

/**
 * `PATCH <path>` with `If-Match: <etag>` and a JSON body, retrying once on
 * an unexpected 401. A stale `etag` — another operator's edit landed on
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
  fetcher: Fetcher = undiciFetch,
): Promise<WithEtag<T>> {
  const url = restUrl(path);
  const payload = JSON.stringify(body);

  const attempt = async (): Promise<UndiciResponse> => {
    const token = await resolveUpstreamAccessToken();
    return fetcher(url, {
      method: "PATCH",
      headers: {
        "content-type": "application/json",
        accept: "application/json",
        authorization: `Bearer ${token}`,
        "if-match": normaliseIfMatch(etag),
      },
      body: payload,
      dispatcher: gatewayAgent(),
    });
  };

  let response = await attempt();
  if (response.status === 401) {
    invalidateUpstreamAccessToken();
    response = await attempt();
  }

  const parsed = await parseGatewayJson(response);
  if (!response.ok) {
    throw mapGatewayError(response.status, parsed, routeLabel);
  }

  return { data: parsed as T, etag: response.headers.get("etag") ?? undefined };
}

/**
 * `POST <path>` with a JSON body — #54's `Route` create, the one write this
 * ticket's screens need that isn't an `If-Match`-guarded edit (a brand-new
 * row has no prior `ETag` to send). Same retry-once-on-401 shape as every
 * other function in this file; `routeLabel` only for `mapGatewayError`'s own
 * log line, same as everywhere else.
 */
export async function postJson<T>(
  path: string,
  body: unknown,
  routeLabel: string,
  fetcher: Fetcher = undiciFetch,
): Promise<T> {
  const url = restUrl(path);
  const payload = JSON.stringify(body);

  const attempt = async (): Promise<UndiciResponse> => {
    const token = await resolveUpstreamAccessToken();
    return fetcher(url, {
      method: "POST",
      headers: {
        "content-type": "application/json",
        accept: "application/json",
        authorization: `Bearer ${token}`,
      },
      body: payload,
      dispatcher: gatewayAgent(),
    });
  };

  let response = await attempt();
  if (response.status === 401) {
    invalidateUpstreamAccessToken();
    response = await attempt();
  }

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
 * **Fixed for the cratestack 0.7.16 bump: this function now acquires an
 * `ETag` and sends it as `If-Match`.** The doc here used to say the
 * generated delete handler needed no `If-Match` at all
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
 * **The shape: a `GET` (via `fetchWithEtag`) to capture the row's current
 * `ETag`, then the `DELETE` carrying it as `If-Match`.** Deliberately kept
 * the function's own signature unchanged — every existing call site
 * (`deleteRoute`, `deleteWebhookEndpoint`, …) keeps working with zero
 * changes, no screen touched, nothing to collide with the console
 * redesign in flight under `admin/`.
 *
 * **Be honest about what this shape costs.** There is a real TOCTOU
 * window between the `GET` and the `DELETE`: if the row is edited in
 * between (another operator's concurrent `PATCH`), the captured `ETag` is
 * stale and the `DELETE` gets a real `412` — that is `If-Match` doing
 * exactly its job, not a bug, but it does mean this shape can fail a
 * delete that would have succeeded a moment earlier. A caller that
 * already holds the row's current `version` — which every screen does,
 * since it necessarily rendered the row to offer a delete button in the
 * first place — could pass it straight through and skip both the extra
 * round trip and the race entirely. That is the better long-term shape;
 * it needs a signature change (`deleteResource` would need an optional
 * `etag`/`version` parameter) and per-screen wiring under `admin/`, which
 * is exactly the work the console redesign already in flight should do
 * for Phase 2, not a reason to leave deletes broken today. This function
 * is the interim: it keeps every delete working now, without touching a
 * single screen.
 *
 * A `404` on the `GET` means the row is already gone — nothing to send
 * `If-Match` against — so the `DELETE` below still runs with no `If-Match`
 * header at all, and its own response is what decides the outcome,
 * unchanged from this function's pre-existing behaviour for a row that
 * doesn't exist. A model with no `@version` field (`OptOut`'s own
 * hand-rolled delete, not routed through this function, is the one
 * example today) never has an `ETag` to capture either, so this adds no
 * behaviour there.
 */
export async function deleteResource(
  path: string,
  routeLabel: string,
  fetcher: Fetcher = undiciFetch,
): Promise<void> {
  const existing = await fetchWithEtag<unknown>(path, routeLabel, fetcher);

  const url = restUrl(path);

  const attempt = async (): Promise<UndiciResponse> => {
    const token = await resolveUpstreamAccessToken();
    const headers: Record<string, string> = {
      accept: "application/json",
      authorization: `Bearer ${token}`,
    };
    if (existing?.etag !== undefined) {
      headers["if-match"] = normaliseIfMatch(existing.etag);
    }
    return fetcher(url, {
      method: "DELETE",
      headers,
      dispatcher: gatewayAgent(),
    });
  };

  let response = await attempt();
  if (response.status === 401) {
    invalidateUpstreamAccessToken();
    response = await attempt();
  }

  if (!response.ok) {
    const parsed = await parseGatewayJson(response);
    throw mapGatewayError(response.status, parsed, routeLabel);
  }
}
