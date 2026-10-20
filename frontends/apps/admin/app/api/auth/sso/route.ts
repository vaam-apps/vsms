/**
 * `GET /api/auth/sso` - where "Sign in with Keycloak" goes.
 *
 * Does for SSO what `GET /login` (middleware) plus `POST /api/auth/login`
 * do for a password: mints the SAME PKCE/state/nonce transaction
 * (`vsms_oidc_txn`, encrypted, HttpOnly, five minutes) and then sends the
 * browser to the gateway - here to `GET /login/sso/start` with the
 * console's authorize request, rather than the gateway's `/login` with a
 * password. A fresh transaction is minted on every click (not borrowed from
 * the cookie `GET /login` set) so a stale one on the login page cannot be
 * the one an SSO attempt completes against.
 *
 * `GET /api/auth/callback` finishes the flow exactly as it does for a
 * password login. This route never sees an upstream token, a code, or a
 * credential, and the console never talks to the identity provider.
 *
 * If the gateway no longer advertises SSO (turned off, or unreachable) the
 * user lands back on `/login` with a message instead of a dead end.
 */
import "server-only";

import { env } from "@vsms/env";
import { redirect } from "next/navigation";
import { generateNonce, generatePkcePair, generateState } from "../../../../lib/oidc";
import { writeTxnCookie } from "../../../../lib/session";
import { buildSsoStartUrl } from "../../../../lib/sso";
import { fetchSsoLoginEndpoint } from "../../../../lib/sso-discovery";

export const runtime = "nodejs";
export const dynamic = "force-dynamic";

export async function GET(): Promise<Response> {
  const endpoint = await fetchSsoLoginEndpoint();
  if (endpoint === undefined) {
    redirect("/login?error=sso_unavailable");
  }

  const { codeVerifier, codeChallenge } = await generatePkcePair();
  const txn = { state: generateState(), nonce: generateNonce(), codeVerifier };
  await writeTxnCookie(txn);

  redirect(
    buildSsoStartUrl(endpoint, {
      clientId: env.SMS_CONSOLE_OIDC_CLIENT_ID,
      redirectUri: new URL("/api/auth/callback", env.ADMIN_BASE_URL).toString(),
      state: txn.state,
      nonce: txn.nonce,
      codeChallenge,
    }),
  );
}
