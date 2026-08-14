import "server-only";

// #211 — the seam that decides WHICH credential an upstream call to
// sms-api presents: the signed-in human's own session token, or this
// console's own machine credential (`token.ts`). Every ordinary gateway
// function (list/get/update screens) goes through `resolveUpstreamAccessToken`
// below; nothing else in this package decides that question on its own.
//
// # Why AsyncLocalStorage, not an explicit parameter threaded through every
// call
//
// `frontends/packages/api/src/context.ts`'s tRPC `Context` already carries
// `gateway: typeof gateway` — a reference to this whole module, not a
// per-request-bound instance — and every router (`frontends/packages/api/src/
// routers/*.ts`) calls `ctx.gateway.listProviders()` etc. with no
// credential argument. Two honest options existed:
//
// 1. Add a credential parameter to every one of this package's ~13
//    upstream-calling functions across 9 files, and to every one of the
//    ~20 router call sites that invoke them. Explicit, and a type error
//    would catch a forgotten one — but it means every future gateway
//    function signature carries a parameter that has nothing to do with
//    what that function actually does, forever, for a decision that is
//    genuinely the same for (almost) the whole request.
// 2. `AsyncLocalStorage`, set once, at the single true per-request
//    boundary (`frontends/apps/admin/app/api/trpc/[trpc]/route.ts`'s `handler`), read
//    implicitly by every call site below it. Idiomatic Node for exactly
//    this shape of problem — "which identity is this request running
//    as" — and it means adding a new gateway function needs no credential
//    plumbing of its own to get this right.
//
// Took option 2. The risk option 2 usually carries — a call site silently
// inheriting whatever credential happened to be ambient, including "none,"
// silently defaulting to the machine credential — is closed by construction
// here: `resolveUpstreamAccessToken` THROWS if no credential was ever set
// for this async context, rather than falling back to anything. A new
// screen's data function that calls it will work correctly the moment it's
// invoked through the ordinary tRPC path (which always sets one — see
// `handler`'s own doc) and will fail loudly, immediately, every time in
// dev and in tests, if it's ever called from somewhere that isn't wrapped
// in `runWithRequestCredential`. That is the property #211 asked for: it
// must be impossible for a new call site to accidentally get the machine
// credential when it meant the human's.
//
// # The two credentials that stay separate on purpose
//
// - **Human** — the signed-in operator's own `Session.accessToken`
//   (`frontends/apps/admin/lib/oidc.ts`), minted by `sms-gateway`'s real
//   `authorization_code` + PKCE flow (#194) under the `sms-console` OIDC
//   client (`GatewayAuth::human_client_id`). `sms_api::auth::GatewayAuth`
//   already validates this token fully — signature, issuer, and audience
//   against `sms-console` — so no exchange step is needed; forwarding it
//   verbatim is correct and, per #211's own framing, "nearly free."
// - **Machine** — `token.ts`'s `SMS_CONSOLE_CLIENT_ID` service-account
//   credential, `client_credentials` + `private_key_jwt`, `kind: "app"`,
//   `role: "app"` always. Reserved for the calls documented at each
//   explicit `getMachineAccessToken()` call site — see `token.ts`'s own
//   module doc for the full list and why each one is there.
//
// # Freshness — no refresh logic lives here
//
// `frontends/apps/admin/middleware.ts` already refreshes a human session's access token
// (`REFRESH_MARGIN_MS`, 60s before expiry) on *every* request, before this
// module ever sees it, and redirects to `/login` outright if refresh fails.
// So by the time a credential reaches `runWithRequestCredential`, it is
// already fresh for the lifetime of that one HTTP request — this module
// has nothing further to do for expiry. See `frontends/apps/admin/lib/oidc.ts` and
// `frontends/apps/admin/middleware.ts`'s own docs for the mechanism.

import { AsyncLocalStorage } from "node:async_hooks";
import { getMachineAccessToken, invalidateMachineAccessToken } from "./token";

export type RequestCredential = { kind: "human"; accessToken: string } | { kind: "machine" };

// The generic includes `undefined` deliberately, not just for the type
// checker's benefit: `storage.run(undefined, fn)` is a real, valid call
// (see `runWithRequestCredential`'s own doc on when the trpc route handler
// passes `undefined`), and `AsyncLocalStorage<RequestCredential>` alone
// (without `| undefined`) rejects that call at compile time even though
// `getStore()` already returns `RequestCredential | undefined` regardless.
const storage = new AsyncLocalStorage<RequestCredential | undefined>();

/**
 * Runs `fn` with `credential` as the ambient upstream identity for every
 * `resolveUpstreamAccessToken()` call made anywhere inside it (including
 * across `await`s — that's the whole point of `AsyncLocalStorage` over a
 * plain module-level variable, which a concurrent request would clobber).
 *
 * `credential === undefined` deliberately does NOT mean "use the machine
 * credential" — it means "no credential decision has been made for this
 * request," and `resolveUpstreamAccessToken` throws rather than guessing.
 * `frontends/apps/admin/app/api/trpc/[trpc]/route.ts`'s `handler` is the one call site
 * that should ever pass `undefined` here, and only when the inbound
 * request is missing the header `middleware.ts` is supposed to have set on
 * every request that reaches this route — a defensive fail-loud, not an
 * expected path.
 */
export function runWithRequestCredential<T>(
  credential: RequestCredential | undefined,
  fn: () => Promise<T>,
): Promise<T> {
  return storage.run(credential, fn);
}

/**
 * The one function every ordinary `@vsms/gateway` call site should use to
 * get a Bearer token for an upstream request. Resolves to the signed-in
 * human's own access token when this call is running inside a
 * `runWithRequestCredential({ kind: "human", ... }, ...)` scope, or to the
 * console's machine credential when running inside a `{ kind: "machine" }`
 * scope (or, for a handful of documented, deliberately-machine-only call
 * sites, callers should import `getMachineAccessToken` from `./token`
 * directly instead of calling this at all — see `token.ts`'s own doc).
 *
 * @throws if called outside any `runWithRequestCredential` scope — see
 * this module's own doc on why that is a hard failure, not a fallback.
 */
export async function resolveUpstreamAccessToken(): Promise<string> {
  const credential = storage.getStore();
  if (credential === undefined) {
    throw new Error(
      "resolveUpstreamAccessToken() called outside a request-credential scope. " +
        "Every upstream call to sms-api must run inside runWithRequestCredential() " +
        "so it is explicit which credential — the signed-in human, or this console's " +
        "own machine credential — it presents. See frontends/packages/gateway/src/" +
        "request-credential.ts's own module doc.",
    );
  }
  return credential.kind === "human" ? credential.accessToken : getMachineAccessToken();
}

/**
 * The retry-on-401 half of every call site's existing "attempt, and on an
 * unexpected 401 invalidate and retry once" shape. For a machine-credential
 * request this genuinely mints a fresh token before the retry (the cache's
 * own `exp - 60s` margin should make this unreachable in normal operation —
 * see `token.ts`'s own doc — but a signing-key rotation invalidating the
 * cached token mid-window is real). For a human-credential request there is
 * nothing cached here to invalidate: the token came from this one request's
 * own session cookie, already refreshed by `frontends/apps/admin/middleware.ts` before the
 * request arrived. A 401 there means the token was rejected for a reason a
 * same-request retry cannot fix (see this module's own freshness doc), so
 * this is a deliberate no-op rather than a second, redundant cache — the
 * retry still happens at the call site, resends the identical token, and
 * gets the identical 401 back, which then surfaces as an ordinary
 * `GatewayError` the same way any other denial does.
 */
export function invalidateUpstreamAccessToken(): void {
  const credential = storage.getStore();
  if (credential === undefined || credential.kind === "machine") {
    invalidateMachineAccessToken();
  }
}
