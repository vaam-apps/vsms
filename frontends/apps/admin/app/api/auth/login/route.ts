/**
 * `POST /api/auth/login` (#194) — the console's own server-side leg of
 * the login form submit. Reads the `vsms_oidc_txn` cookie
 * `frontends/apps/admin/middleware.ts` already set on `GET /login`, calls `sms-gateway`'s
 * own `POST /login` (`backends/apps/sms-gateway/src/login.rs`) server-to-server —
 * the browser never talks to `sms-gateway` directly — and, on success,
 * redirects the browser to the URL that route returned
 * (`{ADMIN_BASE_URL}/api/auth/callback?code=...&state=...`), which is
 * this app's own `GET /api/auth/callback` route below.
 *
 * `state`/`codeChallenge`/`nonce` are read from the (server-side,
 * encrypted, HttpOnly) txn cookie, never trusted from the submitted form —
 * only `email`/`password` come from the request body.
 */
import "server-only";

import { env } from "@vsms/env";
import { redirect } from "next/navigation";
import { readTxnCookie } from "../../../../lib/session";

export const runtime = "nodejs";

export async function POST(request: Request): Promise<Response> {
  const form = await request.formData();
  const email = form.get("email");
  const password = form.get("password");
  if (typeof email !== "string" || typeof password !== "string") {
    redirect("/login?error=invalid_request");
  }

  const txn = await readTxnCookie();
  if (txn === undefined) {
    redirect("/login?error=expired");
  }

  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(txn.codeVerifier));
  const codeChallenge = btoa(String.fromCharCode(...new Uint8Array(digest)))
    .replaceAll("+", "-")
    .replaceAll("/", "_")
    .replaceAll("=", "");

  const redirectUri = new URL("/api/auth/callback", env.ADMIN_BASE_URL).toString();

  let loginResponse: Response;
  try {
    loginResponse = await fetch(new URL("/login", env.SMS_AUTH_ISSUER), {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        email,
        password,
        clientId: env.SMS_CONSOLE_OIDC_CLIENT_ID,
        redirectUri,
        responseType: "code",
        scope: "openid profile",
        state: txn.state,
        codeChallenge,
        codeChallengeMethod: "S256",
        nonce: txn.nonce,
      }),
    });
  } catch (error) {
    console.error("[auth/login] calling sms-gateway /login failed:", error);
    redirect("/login?error=invalid_request");
  }

  if (!loginResponse.ok) {
    const body = (await loginResponse.json().catch(() => ({}))) as { error?: string };
    const code = body.error === "invalid_credentials" ? "invalid_credentials" : "invalid_request";
    redirect(`/login?error=${code}`);
  }

  const body = (await loginResponse.json()) as { redirect?: string };
  if (typeof body.redirect !== "string") {
    console.error("[auth/login] sms-gateway /login returned no redirect:", body);
    redirect("/login?error=invalid_request");
  }

  redirect(body.redirect);
}
