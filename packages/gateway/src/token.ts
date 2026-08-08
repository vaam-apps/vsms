import "server-only";

// `client_credentials` + `private_key_jwt` (RFC 7523) token acquisition
// against `authkestra-op`'s `/token` — the only auth shape this system
// has (#6): no shared client secret exists anywhere. The Next.js app
// authenticates to vsms as an app of its own (the DECISIONS section of
// the architecture plan: "The Next.js app authenticates to vsms as an
// app, with its own client_credentials + private_key_jwt service
// account. User credentials never reach vsms.").
//
// Every write this console ever makes is audited as `SMS_CONSOLE_CLIENT_ID`,
// never as a person — see the dashboard-auth runbook (T9) for the full
// "does not provide" list.
//
// Three things below are load-bearing, each named explicitly in the task:
//
// - `scope` is MANDATORY on the token request. Omitting it does not fall
//   back to the client's registered scopes — it yields `scope: None` on
//   the minted token, and `sms_api::rbac::require_permission` treats a
//   missing scope as denial (Layer 2, #24). `env.SMS_CONSOLE_SCOPE`
//   (`@vsms/env`, default `"sms:send sms:read"`) is always sent.
// - The assertion's `jti` is never reused. `ClientAssertion` is an
//   insert-only table that replay-protects via a `23505` unique violation
//   on `record_jti` (AGENTS.md); reusing a `jti` on retry would collide
//   with the original attempt. `mintAssertion()` below draws a fresh
//   `randomUUID()` on every call, so a retry naturally regenerates the
//   whole assertion rather than resending the same one.
// - The access token is cached until `exp - 60s`, not until `exp` — a
//   60-second safety margin against clock skew and in-flight request
//   latency, so a request never starts with a token that expires before
//   the response comes back.

import { randomUUID } from "node:crypto";
import { readFileSync } from "node:fs";
import { env } from "@vsms/env";
import { type CryptoKey, importPKCS8, SignJWT } from "jose";
import { fetch as undiciFetch } from "undici";
import { gatewayAgent } from "./dispatcher";

const ASSERTION_TTL_SECONDS = 60;
const CLIENT_ASSERTION_TYPE = "urn:ietf:params:oauth:client-assertion-type:jwt-bearer";
const DEFAULT_TOKEN_TTL_SECONDS = 15 * 60;
const EXPIRY_SAFETY_MARGIN_SECONDS = 60;

interface CachedToken {
  accessToken: string;
  /** Epoch milliseconds; the cache is considered stale at or after this. */
  expiresAtMs: number;
}

interface TokenResponse {
  access_token: string;
  token_type?: string;
  expires_in?: number;
  scope?: string;
}

declare global {
  // eslint-disable-next-line no-var
  var __vsmsGatewayTokenCache: CachedToken | undefined;
  // eslint-disable-next-line no-var
  var __vsmsGatewayTokenInFlight: Promise<string> | undefined;
  // eslint-disable-next-line no-var
  var __vsmsGatewaySigningKey: Promise<CryptoKey> | undefined;
}

function tokenEndpoint(): string {
  return new URL("/token", env.SMS_AUTH_ISSUER).toString();
}

function signingKey(): Promise<CryptoKey> {
  globalThis.__vsmsGatewaySigningKey ??= (async () => {
    const pem = readFileSync(env.SMS_CONSOLE_PRIVATE_KEY_PATH, "utf8");
    const key = await importPKCS8(pem, "RS256");
    return key;
  })();
  return globalThis.__vsmsGatewaySigningKey;
}

/**
 * A fresh RFC 7523 §3 client assertion, signed with the console's own
 * private key. `kid` is the client id — `authkestra_op`'s
 * `select_key` treats a single-key JWKS (which is all `provisionAppClient`
 * ever produces, per AGENTS.md) as unambiguous even without a `kid`, but
 * setting it costs nothing and matches what a real client would do.
 */
async function mintAssertion(): Promise<string> {
  const key = await signingKey();
  const now = Math.floor(Date.now() / 1000);
  return await new SignJWT({})
    .setProtectedHeader({ alg: "RS256", kid: env.SMS_CONSOLE_CLIENT_ID })
    .setIssuer(env.SMS_CONSOLE_CLIENT_ID)
    .setSubject(env.SMS_CONSOLE_CLIENT_ID)
    .setAudience(tokenEndpoint())
    .setJti(randomUUID())
    .setIssuedAt(now)
    .setExpirationTime(now + ASSERTION_TTL_SECONDS)
    .sign(key);
}

async function requestToken(): Promise<CachedToken> {
  const assertion = await mintAssertion();
  const body = new URLSearchParams({
    grant_type: "client_credentials",
    client_id: env.SMS_CONSOLE_CLIENT_ID,
    client_assertion_type: CLIENT_ASSERTION_TYPE,
    client_assertion: assertion,
    // Mandatory — see module doc above.
    scope: env.SMS_CONSOLE_SCOPE,
  });

  const response = await undiciFetch(tokenEndpoint(), {
    method: "POST",
    headers: { "content-type": "application/x-www-form-urlencoded" },
    body: body.toString(),
    dispatcher: gatewayAgent(),
  });

  const text = await response.text();
  if (!response.ok) {
    throw new Error(`token request to ${tokenEndpoint()} failed (${response.status}): ${text}`);
  }

  const parsed = JSON.parse(text) as TokenResponse;
  const ttlSeconds = parsed.expires_in ?? DEFAULT_TOKEN_TTL_SECONDS;
  return {
    accessToken: parsed.access_token,
    expiresAtMs: Date.now() + Math.max(ttlSeconds - EXPIRY_SAFETY_MARGIN_SECONDS, 0) * 1000,
  };
}

/**
 * A valid Bearer access token for calling sms-api, minting and caching a
 * new one only when the cached one is within `EXPIRY_SAFETY_MARGIN_SECONDS`
 * of expiry. Concurrent callers during a cache miss share one in-flight
 * request rather than each minting their own token (and burning their own
 * `jti`) simultaneously.
 */
export async function getAccessToken(): Promise<string> {
  const cached = globalThis.__vsmsGatewayTokenCache;
  if (cached != null && cached.expiresAtMs > Date.now()) {
    return cached.accessToken;
  }

  globalThis.__vsmsGatewayTokenInFlight ??= (async () => {
    try {
      const token = await requestToken();
      globalThis.__vsmsGatewayTokenCache = token;
      return token.accessToken;
    } finally {
      globalThis.__vsmsGatewayTokenInFlight = undefined;
    }
  })();

  return globalThis.__vsmsGatewayTokenInFlight;
}

/**
 * Drops the cached access token. `client.ts` calls this once on an
 * unexpected 401 from sms-api before retrying — the cache's own
 * `exp - 60s` margin should make that unreachable in normal operation, but
 * a signing-key rotation invalidating the cached token mid-window is a
 * real scenario this guards against without waiting out the full TTL.
 */
export function invalidateAccessToken(): void {
  globalThis.__vsmsGatewayTokenCache = undefined;
}
