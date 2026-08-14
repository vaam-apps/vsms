import "server-only";

// Per-request tRPC context (Seam 1 — browser <-> Next.js, architecture
// plan §4). Two responsibilities, both load-bearing:
//
// 1. Read the `x-vsms-actor` header, if present, as the human display
//    name for local audit/UI purposes only. This is *not* an identity
//    that reaches sms-api — per the DECISIONS section of the
//    architecture plan, dashboard auth (T9, none|basic) is entirely
//    local to Next.js, and every upstream write is audited as
//    `SMS_CONSOLE_CLIENT_ID`, never as a person. `ctx.actor` exists so a
//    future screen can show "sent by alice" in the console's own UI
//    without pretending sms-api knows who alice is.
//
//    T9 (the auth toggle) is a separate, not-yet-landed task. Its own
//    brief is explicit that its Basic-auth middleware must "copy headers
//    and delete('x-vsms-actor') FIRST" before setting its own value —
//    otherwise a caller could simply assert any actor by sending the
//    header itself. Until that middleware exists, this context trusts
//    the header as-is (there is nothing upstream to strip a forged one),
//    which is safe today only because `DASHBOARD_AUTH` defaults to
//    `none` and nothing downstream treats `actor` as an authorization
//    decision — it is display-only, never passed to sms-api.
//
// 2. An Origin check on POST — the console has no CSRF token (T9's own
//    "does not provide" list names this: "weak CSRF posture (preflight +
//    Origin check, not a token)"). Every tRPC mutation arrives as a POST,
//    so this is the one control this seam has: a cross-site POST will
//    carry an `Origin` header that doesn't match the request's own
//    origin. A POST carrying NO `Origin` at all is also refused: modern
//    browsers reliably send it on a fetch POST regardless of same- or
//    cross-origin, so the only callers that omit it are non-browser
//    clients — precisely the case this control exists to stop. This
//    endpoint reaches `sendMessage`, which sends a real billed SMS, so
//    absence is treated as refusal rather than permission.
//
//    (An earlier revision of this file said the opposite, and the code
//    matched it by returning early on a missing header. That was a real
//    hole — plain `curl` with no headers bypassed the check entirely.
//    Both were fixed; this note exists so the doc is never "corrected"
//    back toward the unsafe behaviour.)

import { TRPCError } from "@trpc/server";
import type { FetchCreateContextFnOptions } from "@trpc/server/adapters/fetch";
import { env } from "@vsms/env";
import * as gateway from "@vsms/gateway";

const ACTOR_HEADER = "x-vsms-actor";

/**
 * The origin a browser is genuinely expected to reach this console at.
 *
 * **Not** `new URL(req.url).origin`, which is what this check used until
 * #243 and which is wrong the moment the console runs in a container.
 * Measured, not inferred: a Next.js 15 standalone server behind a reverse
 * proxy reports `req.url` as `https://0.0.0.0:3000/...` — the host comes
 * from the server's own `HOSTNAME`/`PORT` bind, and neither `Host` nor
 * `X-Forwarded-Host` influences it (`X-Forwarded-Proto` does reach the
 * scheme, which is why the old value looked half-plausible). No browser
 * can ever send `https://0.0.0.0:3000` as its `Origin`, so every POST —
 * i.e. every mutation in the console — was rejected in the production
 * compose stack and under the Helm chart. Reads were unaffected, which is
 * part of why it survived: the check returns early on anything but POST.
 *
 * `ADMIN_BASE_URL` is the right source of truth and needs no new trust.
 * `@vsms/env` already validates it as a URL at startup, `deploy/docker-
 * compose.yml` already documents it as "the public console origin a
 * browser actually reaches", and `sms-gateway seed-console-client`
 * already registers the OIDC `redirect_uri` against it — compared as a
 * whole string per RFC 6749 3.1.2, so a deployment with this value wrong
 * cannot complete a login at all. A console that can log in therefore has
 * this value right, by construction.
 *
 * Deliberately not `X-Forwarded-Host`: that would mean trusting a header
 * an attacker controls to decide what counts as same-origin, which
 * inverts the control. #163 already reasoned this repo cannot blanket-
 * trust `X-Forwarded-*` anyway — `deploy/docker-compose.yml` has two
 * internal callers of the gateway, not one, so there is no single trusted
 * hop to pin.
 *
 * **Computed lazily, not at module scope.** The first version of this
 * evaluated `new URL(env.ADMIN_BASE_URL)` as a module-level `const` and
 * broke `next build` with `TypeError: Invalid URL` while collecting page
 * data for `/api/trpc/[trpc]`: CI builds with `SKIP_ENV_VALIDATION=true`
 * (there is no real upstream to point at during a build), so
 * `ADMIN_BASE_URL` is `undefined` there and `new URL(undefined)` throws.
 * It passed locally only because the developer's shell happened to have
 * the variable set. Next's build-time page-data collection imports this
 * module without ever serving a request, so anything evaluated at module
 * scope must hold for a build with no runtime configuration at all.
 * Deferring to first use keeps the failure where it belongs: on a real
 * request, in a real deployment, where the value is genuinely required.
 */
let expectedOrigin: string | undefined;

function expectedConsoleOrigin(): string {
  if (expectedOrigin === undefined) {
    expectedOrigin = new URL(env.ADMIN_BASE_URL).origin;
  }
  return expectedOrigin;
}

export interface Context {
  /** Display-only local actor name — see module doc. Never sent upstream. */
  actor: string | null;
  gateway: typeof gateway;
}

function assertSameOriginForMutations(req: Request): void {
  if (req.method !== "POST") return;

  const origin = req.headers.get("origin");
  // A POST with no Origin at all is NOT trusted. Browsers always set Origin on
  // a fetch POST, so the only callers that omit it are non-browser clients —
  // which is exactly the case an early `return` here waved through. This
  // endpoint triggers a real `sendMessage`, so the default must be refusal.
  if (origin === null) {
    throw new TRPCError({
      code: "FORBIDDEN",
      message: "cross-origin request rejected: POST requires an Origin header",
    });
  }

  const expected = expectedConsoleOrigin();
  if (origin !== expected) {
    throw new TRPCError({
      code: "FORBIDDEN",
      message: `cross-origin request rejected: Origin "${origin}" does not match "${expected}"`,
    });
  }
}

export function createContext({ req }: FetchCreateContextFnOptions): Context {
  assertSameOriginForMutations(req);
  return {
    actor: req.headers.get(ACTOR_HEADER),
    gateway,
  };
}
