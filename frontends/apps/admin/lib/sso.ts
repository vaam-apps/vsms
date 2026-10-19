/**
 * The console's half of single sign-on: pure helpers, Edge-and-Node-portable
 * and importing nothing server-only, for the same reason `./oidc.ts` is -
 * the login page, the `GET /api/auth/sso` route and the callback route all
 * need to agree, and each is tested here without a framework.
 *
 * **What SSO is, from the console's side: nothing new.** The console never
 * talks to the staff identity provider (Keycloak). The gateway is still the
 * console's OP; "Sign in with Keycloak" is just another way to reach the
 * gateway's authorize step. So the console does exactly what the password
 * path does - mint the PKCE/state/nonce transaction (`vsms_oidc_txn`) - and
 * then, instead of POSTing credentials to `sms-gateway`'s `/login`, sends the
 * *browser* to `sms-gateway`'s `GET /login/sso/start` with the same authorize
 * request. `GET /api/auth/callback` then completes it unchanged: same `state`
 * check, same `/token` exchange with the same verifier, same `id_token`
 * verification (signature, `iss`, `aud`, `exp`, `nonce`).
 *
 * **How the console learns SSO exists.** From the gateway's OIDC discovery
 * document, which carries a `vsms_sso_login_endpoint` member exactly when the
 * gateway has SSO configured. One authority, no console env var to drift out
 * of step with the gateway's (and R4 holds: nothing here is required).
 */

/** The discovery-document member the gateway publishes when SSO is on. */
export const SSO_DISCOVERY_FIELD = "vsms_sso_login_endpoint";

/** The path the gateway mounts its SSO entry point at. */
const SSO_START_PATH = "/login/sso/start";

/**
 * The SSO start URL out of a discovery document, or `undefined` if the
 * document does not offer one - or offers something that is not a plausible
 * gateway start URL. The value came from our own gateway, but it becomes a
 * redirect target, so it is held to: an absolute `http(s)` URL whose path is
 * exactly the start path, with no query, fragment or credentials.
 */
export function ssoLoginEndpointFrom(discovery: unknown): string | undefined {
  if (typeof discovery !== "object" || discovery === null) return undefined;
  const value = (discovery as Record<string, unknown>)[SSO_DISCOVERY_FIELD];
  if (typeof value !== "string") return undefined;
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    return undefined;
  }
  if (url.protocol !== "https:" && url.protocol !== "http:") return undefined;
  if (url.pathname !== SSO_START_PATH) return undefined;
  if (url.search !== "" || url.hash !== "") return undefined;
  if (url.username !== "" || url.password !== "") return undefined;
  return url.toString();
}

export interface SsoStartParams {
  clientId: string;
  redirectUri: string;
  state: string;
  nonce: string;
  codeChallenge: string;
}

/**
 * The console's authorize request as the query string of the gateway's SSO
 * start URL - the same eight values the password path posts as JSON to
 * `/login`, with the same scope.
 */
export function buildSsoStartUrl(endpoint: string, params: SsoStartParams): string {
  const url = new URL(endpoint);
  url.searchParams.set("client_id", params.clientId);
  url.searchParams.set("redirect_uri", params.redirectUri);
  url.searchParams.set("response_type", "code");
  url.searchParams.set("scope", "openid profile");
  url.searchParams.set("state", params.state);
  url.searchParams.set("nonce", params.nonce);
  url.searchParams.set("code_challenge", params.codeChallenge);
  url.searchParams.set("code_challenge_method", "S256");
  return url.toString();
}

/**
 * Maps the OAuth `error` the gateway redirects back to `/api/auth/callback`
 * with onto a `/login?error=` code. The gateway sends only two: a generic
 * `access_denied` for every refusal (deliberately - nothing in it helps
 * enumerate accounts or groups) and `temporarily_unavailable` when the
 * identity provider or database could not be used. Anything else is treated
 * as the generic refusal rather than echoed.
 */
export function loginErrorForOauthError(error: string): "sso_failed" | "sso_unavailable" {
  return error === "temporarily_unavailable" || error === "server_error"
    ? "sso_unavailable"
    : "sso_failed";
}
