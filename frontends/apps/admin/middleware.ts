/**
 * The dashboard's authentication gate (#194) — a human session, not
 * `DASHBOARD_AUTH=none|basic`.
 *
 * **Hard cutover, not a parallel path.** The previous revision of this
 * file (#48) gated on HTTP Basic auth against a `DASHBOARD_BASIC_USERS`
 * env list — its own module doc was explicit about what that did and
 * didn't provide: "Basic-auth users are not `User` rows and carry no
 * role... SHA-256 is not a password hash... it is not a production
 * human-auth story." #194 is that production story, and per this
 * project's own standing convention (replace, don't run both — see
 * `AGENTS.md`), Basic auth is gone from this file entirely: no
 * `DASHBOARD_AUTH` env var, no fallback, no flag to re-enable it. If a
 * deployment somehow needs it back, that's a revert of this commit, not a
 * toggle.
 *
 * # What this now provides that #48 explicitly said it did not
 *
 * - **Real identity.** A session traces back to a `User` row via
 *   `sms_auth::login::authenticate_user` (`backends/apps/sms-gateway/src/login.rs`) —
 *   `x-vsms-actor` below is that row's own `email`, not an operator-typed
 *   Basic-auth username with no backing account.
 * - **Real roles**, carried in the session and forwarded as
 *   `x-vsms-role` — display-only in this file (see `x-vsms-actor`'s own
 *   note); the actual authorization decision lives at `sms-api`, driven by
 *   the *token* forwarded below, not by this header.
 * - **The signed-in human's own upstream credential** (#211), forwarded as
 *   `x-vsms-access-token` — every upstream call to `sms-gateway` used to
 *   authenticate exclusively as the shared `SMS_CONSOLE_CLIENT_ID` machine
 *   credential (`frontends/packages/gateway/src/token.ts`) regardless of who was
 *   signed in; #211 closed that gap. This header is how the human's own
 *   already-validated, already-freshness-checked `session.accessToken`
 *   (see `refreshSession`'s own doc immediately below) crosses from this
 *   middleware into `frontends/apps/admin/app/api/trpc/[trpc]/route.ts`, the one place
 *   that reads it and opens a `runWithRequestCredential` scope
 *   (`@vsms/gateway`) for the rest of that request. Same mechanism
 *   `x-vsms-actor`/`x-vsms-role` already established — Next.js forwards a
 *   header set here on the *request* object to every downstream Server
 *   Component/Route Handler in the same request, no network hop involved.
 * - **Real logout and expiry.** `POST /api/auth/logout` clears the
 *   session cookie outright; the cookie itself carries a real expiry
 *   (`SESSION_COOKIE_MAX_AGE_SECONDS`, §5.3's own 8h human-refresh-token
 *   figure) and this middleware refreshes or expires it on a schedule
 *   tied to the OP's own token lifetimes, not "until someone edits an env
 *   var and restarts."
 *
 * # What this still does not provide
 *
 * - **Brute-force protection on `/login` itself.** `sms-gateway`'s own
 *   `/login` route has no rate limiting of its own yet — the same class
 *   of gap `#156`/`#168` closed for `/token`, not yet closed here. Tracked
 *   as explicit follow-up in this PR's own description, not silently
 *   assumed covered by Caddy's existing `/token` zones (which don't match
 *   this path).
 * - **A CSRF token on the login form POST.** `POST /api/auth/login` is a
 *   plain `<form>` submission (see `frontends/apps/admin/app/login/page.tsx`) rather than
 *   a fetch-based one, so it has no `Origin` header the way
 *   `frontends/packages/api/src/context.ts`'s own tRPC mutations do to check
 *   against. Its blast radius if forged cross-site is bounded — a forged
 *   submission can only ever attempt *someone else's own* credentials
 *   against `/login`, never act on an existing session — but a same-site
 *   token would still be a real improvement, and isn't built here.
 *
 * # Why the PKCE/state/nonce transaction cookie is minted here, not in
 * the `/login` page component
 *
 * Next.js only allows `cookies().set()` inside a Server Action, a Route
 * Handler, or Middleware — never during a plain Server Component render,
 * which is what `frontends/apps/admin/app/login/page.tsx` otherwise is. So *this* file
 * mints the transaction (`state`, `nonce`, PKCE `codeVerifier`/
 * `codeChallenge`) the moment a `GET /login` request arrives, sets the
 * encrypted `vsms_oidc_txn` cookie, and only then lets the request reach
 * the page component — which renders a form that already knows nothing
 * about any of this; `POST /api/auth/login` reads the cookie back
 * server-side instead of trusting anything the form itself submits for
 * `state`/`nonce`/`codeChallenge`.
 */
import { type NextRequest, NextResponse } from "next/server";
import {
  decryptSession,
  encryptSession,
  encryptTxn,
  generateNonce,
  generatePkcePair,
  generateState,
  OIDC_COOKIE_NAMES,
  SESSION_COOKIE_MAX_AGE_SECONDS,
  type Session,
  TXN_COOKIE_MAX_AGE_SECONDS,
} from "./lib/oidc";

const ACTOR_HEADER = "x-vsms-actor";
const ROLE_HEADER = "x-vsms-role";
/** #211 — see this file's own module doc. Carries the signed-in human's
 * real, freshness-checked OAuth access token one hop downstream, in
 * process, to the tRPC route handler. */
const ACCESS_TOKEN_HEADER = "x-vsms-access-token";

/** Same exclusions as before #194 — `api/health` still needs to answer an
 * unauthenticated liveness probe (#139's own finding, still true: a
 * container `HEALTHCHECK` has no session cookie to present). `/login` and
 * `/api/auth/*` are newly reachable without a session — they're the only
 * way to *get* one. */
export const config = {
  matcher: [
    "/((?!_next/static|_next/image|favicon.ico|icons/|manifest.webmanifest|sw.js|api/health).*)",
  ],
};

function sessionSecret(): string {
  const secret = process.env.SMS_CONSOLE_SESSION_SECRET;
  if (secret === undefined || secret.length < 32) {
    // Same posture as @vsms/env's own startup validation for this var —
    // duplicated here (not imported from @vsms/env) because middleware
    // runs on the Edge runtime and this file, unlike route handlers,
    // predates any confidence that @vsms/env's full zod schema is Edge-safe
    // end to end; a raw process.env read matches this file's own
    // pre-#194 convention (it never imported @vsms/env either).
    throw new Error("SMS_CONSOLE_SESSION_SECRET must be set and at least 32 characters");
  }
  return secret;
}

async function ensureLoginTxnCookie(response: NextResponse): Promise<void> {
  const { codeVerifier, codeChallenge } = await generatePkcePair();
  const token = await encryptTxn(
    { state: generateState(), nonce: generateNonce(), codeVerifier },
    sessionSecret(),
  );
  response.cookies.set(OIDC_COOKIE_NAMES.txn, token, {
    httpOnly: true,
    secure: process.env.NODE_ENV === "production",
    sameSite: "lax",
    path: "/",
    maxAge: TXN_COOKIE_MAX_AGE_SECONDS,
  });
  // codeChallenge itself is not secret and has no reason to round-trip
  // through a second cookie — frontends/apps/admin/app/login/page.tsx never needs it;
  // POST /api/auth/login reads codeVerifier back out of the txn cookie
  // above and recomputes the identical challenge from it server-side.
  void codeChallenge;
}

/** Access-token refresh, done here (not in a route handler) so it runs
 * ahead of *every* request, not just the ones a user happens to trigger a
 * page load on: a session whose access token is within `REFRESH_MARGIN_MS`
 * of expiry gets a fresh one via the real `authorization_code` flow's
 * sibling grant, `refresh_token` — `authkestra_op`'s own
 * `handle_refresh_token`, unmodified. Failure (network, expired refresh
 * token, revoked client) means the session cannot continue: redirect to
 * `/login`, same as no session at all. */
const REFRESH_MARGIN_MS = 60_000;

async function refreshSession(session: Session): Promise<Session | undefined> {
  if (session.refreshToken === null) return undefined;
  const issuer = process.env.SMS_AUTH_ISSUER;
  if (issuer === undefined) return undefined;

  const body = new URLSearchParams({
    grant_type: "refresh_token",
    refresh_token: session.refreshToken,
    client_id: process.env.SMS_CONSOLE_OIDC_CLIENT_ID ?? "sms-console",
  });
  let response: Response;
  try {
    response = await fetch(new URL("/token", issuer), {
      method: "POST",
      headers: { "content-type": "application/x-www-form-urlencoded" },
      body: body.toString(),
    });
  } catch {
    return undefined;
  }
  if (!response.ok) return undefined;

  const parsed = (await response.json()) as {
    access_token?: string;
    refresh_token?: string;
    expires_in?: number;
  };
  if (parsed.access_token === undefined) return undefined;

  return {
    ...session,
    accessToken: parsed.access_token,
    refreshToken: parsed.refresh_token ?? session.refreshToken,
    accessTokenExpiresAtMs: Date.now() + (parsed.expires_in ?? 900) * 1000,
  };
}

function redirectToLogin(request: NextRequest): NextResponse {
  const response = NextResponse.redirect(new URL("/login", request.url));
  response.cookies.delete(OIDC_COOKIE_NAMES.session);
  return response;
}

export async function middleware(request: NextRequest): Promise<NextResponse> {
  const path = request.nextUrl.pathname;

  // Strip any inbound actor/role/access-token headers BEFORE anything else
  // — same reasoning #48's own version of this file already established for
  // x-vsms-actor: downstream code trusts these, so a caller that could set
  // them directly would be asserting whatever identity (or, for
  // ACCESS_TOKEN_HEADER, whatever credential) it liked. A forged token
  // value alone can't forge a *valid* signature — `GatewayAuth` still
  // verifies it against the real JWKS — but stripping it here means
  // nothing downstream can even be tempted to trust an unvalidated
  // caller-supplied value for it.
  const headers = new Headers(request.headers);
  headers.delete(ACTOR_HEADER);
  headers.delete(ROLE_HEADER);
  headers.delete(ACCESS_TOKEN_HEADER);

  if (path.startsWith("/api/auth/")) {
    // These routes manage vsms_oidc_txn/vsms_session themselves — nothing
    // to gate or inject here.
    return NextResponse.next({ request: { headers } });
  }

  if (path === "/login" && request.method === "GET") {
    const response = NextResponse.next({ request: { headers } });
    await ensureLoginTxnCookie(response);
    return response;
  }
  if (path === "/login") {
    return NextResponse.next({ request: { headers } });
  }

  const raw = request.cookies.get(OIDC_COOKIE_NAMES.session)?.value;
  let session = raw === undefined ? undefined : await decryptSession(raw, sessionSecret());
  if (session === undefined) {
    return redirectToLogin(request);
  }

  if (session.accessTokenExpiresAtMs - Date.now() < REFRESH_MARGIN_MS) {
    const refreshed = await refreshSession(session);
    if (refreshed === undefined) {
      return redirectToLogin(request);
    }
    session = refreshed;
  }

  headers.set(ACTOR_HEADER, session.email);
  headers.set(ROLE_HEADER, session.role);
  headers.set(ACCESS_TOKEN_HEADER, session.accessToken);
  const response = NextResponse.next({ request: { headers } });
  const token = await encryptSession(session, sessionSecret());
  response.cookies.set(OIDC_COOKIE_NAMES.session, token, {
    httpOnly: true,
    secure: process.env.NODE_ENV === "production",
    sameSite: "lax",
    path: "/",
    maxAge: SESSION_COOKIE_MAX_AGE_SECONDS,
  });
  return response;
}
