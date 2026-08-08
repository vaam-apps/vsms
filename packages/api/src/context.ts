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
import * as gateway from "@vsms/gateway";

const ACTOR_HEADER = "x-vsms-actor";

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

  const expected = new URL(req.url).origin;
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
