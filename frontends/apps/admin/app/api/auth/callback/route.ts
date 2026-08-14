/**
 * `GET /api/auth/callback` (#194) — where `#194`'s own hard requirements
 * ("PKCE... state and nonce, both verified on the callback") are actually
 * enforced. `code`/`state` arrive as query parameters, exactly the way a
 * real OAuth2 authorization-code redirect delivers them (see
 * `backends/apps/sms-gateway/src/login.rs`'s own doc for why *this* app is the one
 * that receives a genuine HTTP redirect, not `sms-gateway`'s `/login`
 * itself, which returns JSON to `POST /api/auth/login` instead).
 *
 * Five things happen here, in order, each a hard stop on failure:
 *
 * 1. Read the `vsms_oidc_txn` cookie set at `GET /login` — if it's
 *    missing/expired/undecryptable, the attempt cannot be completed.
 * 2. **Verify `state`** (`verifyState`, `../../../../lib/oidc.ts`) against
 *    what the cookie says this browser's own login attempt generated.
 * 3. Exchange `code` for tokens at `sms-gateway`'s real `/token`, with
 *    `code_verifier` from the same cookie — `authkestra_op`'s own
 *    `handle_authorization_code`, unmodified, is what actually enforces
 *    PKCE S256 here; this route just supplies the verifier it already had.
 * 4. **Verify the `id_token`**: signature (via `sms-gateway`'s own
 *    `/jwks.json`), `iss`, `aud`, `exp`, and — **`verifyNonce`** — the
 *    `nonce` claim against the same cookie's own value.
 * 5. Establish the session (`writeSessionCookie`) and redirect to `/`.
 */
import "server-only";

import { env } from "@vsms/env";
import { createRemoteJWKSet, jwtVerify } from "jose";
import { redirect } from "next/navigation";
import { verifyNonce, verifyState } from "../../../../lib/oidc";
import { clearTxnCookie, readTxnCookie, writeSessionCookie } from "../../../../lib/session";

export const runtime = "nodejs";

interface TokenResponse {
  access_token: string;
  refresh_token?: string;
  id_token?: string;
  expires_in?: number;
}

interface IdTokenClaims {
  sub: string;
  email?: string;
  name?: string;
  nonce?: string;
}

function jwks() {
  return createRemoteJWKSet(new URL("/jwks.json", env.SMS_AUTH_ISSUER));
}

export async function GET(request: Request): Promise<Response> {
  const url = new URL(request.url);
  const code = url.searchParams.get("code");
  const returnedState = url.searchParams.get("state");

  const txn = await readTxnCookie();
  if (txn === undefined || code === null || returnedState === null) {
    redirect("/login?error=expired");
  }

  // The state check.
  if (!verifyState(returnedState, txn.state)) {
    console.warn("[auth/callback] state mismatch — refusing the callback");
    await clearTxnCookie();
    redirect("/login?error=invalid_request");
  }

  const redirectUri = new URL("/api/auth/callback", env.ADMIN_BASE_URL).toString();
  const tokenResponse = await fetch(new URL("/token", env.SMS_AUTH_ISSUER), {
    method: "POST",
    headers: { "content-type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({
      grant_type: "authorization_code",
      code,
      redirect_uri: redirectUri,
      client_id: env.SMS_CONSOLE_OIDC_CLIENT_ID,
      code_verifier: txn.codeVerifier,
    }).toString(),
  });

  if (!tokenResponse.ok) {
    console.error(
      "[auth/callback] /token exchange failed:",
      tokenResponse.status,
      await tokenResponse.text(),
    );
    await clearTxnCookie();
    redirect("/login?error=invalid_request");
  }

  const tokens = (await tokenResponse.json()) as TokenResponse;
  if (tokens.id_token === undefined) {
    console.error("[auth/callback] /token response carried no id_token");
    await clearTxnCookie();
    redirect("/login?error=invalid_request");
  }

  let claims: IdTokenClaims;
  try {
    const { payload } = await jwtVerify(tokens.id_token, jwks(), {
      issuer: env.SMS_AUTH_ISSUER,
      audience: env.SMS_CONSOLE_OIDC_CLIENT_ID,
    });
    claims = payload as unknown as IdTokenClaims;
  } catch (error) {
    console.error("[auth/callback] id_token signature/iss/aud/exp verification failed:", error);
    await clearTxnCookie();
    redirect("/login?error=invalid_request");
  }

  // The nonce check.
  if (!verifyNonce(claims.nonce, txn.nonce)) {
    console.warn("[auth/callback] id_token nonce mismatch — refusing the callback");
    await clearTxnCookie();
    redirect("/login?error=invalid_request");
  }

  await writeSessionCookie({
    accessToken: tokens.access_token,
    refreshToken: tokens.refresh_token ?? null,
    accessTokenExpiresAtMs: Date.now() + (tokens.expires_in ?? 900) * 1000,
    subject: claims.sub,
    email: claims.email ?? "",
    displayName: claims.name ?? "",
    // sms_api::auth::GatewayAuth resolves role/perms per request from
    // User/Role (see that module's own doc on why the real
    // authkestra-op library shape ruled out baking them into the token
    // at issuance) — the id_token itself carries no role claim to read
    // here. admin's own display of "which role am I" is cosmetic only
    // (Layer 3, §5.1) and not yet wired to a real per-user source; left
    // blank rather than guessed, and flagged as a natural follow-up
    // alongside #58's own users-and-roles screens.
    role: "",
  });
  await clearTxnCookie();

  redirect("/");
}
