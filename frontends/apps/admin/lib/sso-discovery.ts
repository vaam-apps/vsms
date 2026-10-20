import "server-only";

/**
 * Asks the gateway whether SSO is on, by reading its OIDC discovery document
 * (see `./sso.ts` for why that is the source of truth).
 *
 * Server-only: this runs in the login page's server render and in
 * `GET /api/auth/sso`, using `SMS_AUTH_ISSUER` - which may be an internal
 * address - for the *fetch*. The URL it returns is the gateway's own public
 * one, from the document, and is what the *browser* is sent to.
 *
 * **Never throws and never blocks a login.** The password form must render
 * even when the gateway is slow or the document is odd: a failure is
 * `undefined` ("no SSO button"), bounded by a short timeout. The result is
 * cached for a few seconds so a burst of login-page renders is one request,
 * not one each.
 */
import { env } from "@vsms/env";
import { ssoLoginEndpointFrom } from "./sso";

const TIMEOUT_MS = 2000;
const CACHE_MS = 10_000;

let cached: { endpoint: string | undefined; at: number } | undefined;

export async function fetchSsoLoginEndpoint(): Promise<string | undefined> {
  if (cached !== undefined && Date.now() - cached.at < CACHE_MS) {
    return cached.endpoint;
  }
  let endpoint: string | undefined;
  try {
    const response = await fetch(
      new URL("/.well-known/openid-configuration", env.SMS_AUTH_ISSUER),
      {
        cache: "no-store",
        signal: AbortSignal.timeout(TIMEOUT_MS),
      },
    );
    if (response.ok) {
      endpoint = ssoLoginEndpointFrom(await response.json());
    }
  } catch {
    endpoint = undefined;
  }
  cached = { endpoint, at: Date.now() };
  return endpoint;
}
