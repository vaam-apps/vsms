/**
 * The console's own OIDC authorization-code + PKCE client half (#194).
 *
 * Pure, Edge-and-Node-portable helpers — no `server-only`, no `@vsms/env`
 * import at module scope. Deliberately so: `admin/middleware.ts` (Edge
 * runtime, per Next.js — middleware cannot opt into the Node runtime) needs
 * the session-cookie decrypt/verify functions here on every request, and
 * the route handlers under `admin/app/api/auth/` (Node runtime) need the
 * same encrypt/decrypt logic to *agree* with what middleware reads — one
 * shared module, not two copies that could drift. `jose` (already a
 * dependency of `@vsms/gateway`, which this package already depends on —
 * see `packages/gateway/src/token.ts`) is built specifically to run
 * identically under both: WebCrypto where available (Edge, browsers),
 * Node's `crypto` module otherwise, one API either way. Hand-rolling
 * AES-GCM directly here was considered and rejected — this is exactly the
 * kind of security-critical code (nonce/IV handling, authenticated
 * encryption) a maintained library gets right more reliably than a
 * one-off implementation would.
 *
 * # Two cookies, two purposes
 *
 * - **`vsms_oidc_txn`** — short-lived (5 minutes), holds the PKCE
 *   `codeVerifier`, `state`, and `nonce` a login attempt generated at
 *   `GET /login` and needs again at `POST /api/auth/login` (to send
 *   `state`/`codeChallenge` to `sms-gateway`'s own `/login`) and
 *   `GET /api/auth/callback` (to verify the returned `state` and complete
 *   the PKCE exchange with `codeVerifier`).
 * - **`vsms_session`** — the actual human session, holding the OP-issued
 *   `accessToken`/`refreshToken`/`expiresAt` plus the small set of claims
 *   `admin`'s own UI needs to display (`email`, `role`). Read on every
 *   request by `middleware.ts` to decide authenticated-vs-not; refreshed
 *   there too when the access token is close to expiry (see
 *   `admin/middleware.ts`'s own doc).
 *
 * Both are `jose` `EncryptJWT`/`jwtDecrypt` (JWE, direct A256GCM) —
 * encrypted, not merely signed, because `vsms_session` carries a real
 * bearer access token. `HttpOnly`/`Secure`/`SameSite=Lax` are applied
 * where the cookie is *set* (the route handlers / middleware), not here —
 * this module only produces and consumes the token string.
 */

import { EncryptJWT, jwtDecrypt } from "jose";

const TXN_COOKIE_NAME = "vsms_oidc_txn";
const SESSION_COOKIE_NAME = "vsms_session";
const TXN_TTL_SECONDS = 5 * 60;

export const OIDC_COOKIE_NAMES = {
  txn: TXN_COOKIE_NAME,
  session: SESSION_COOKIE_NAME,
} as const;

export interface OidcTxn {
  state: string;
  nonce: string;
  codeVerifier: string;
}

export interface Session {
  accessToken: string;
  refreshToken: string | null;
  /** Epoch milliseconds — when `accessToken` itself expires. */
  accessTokenExpiresAtMs: number;
  subject: string;
  email: string;
  displayName: string;
  role: string;
}

function base64UrlEncode(bytes: Uint8Array): string {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary).replaceAll("+", "-").replaceAll("/", "_").replaceAll("=", "");
}

/** A cryptographically random, URL-safe token — `byteLength` bytes of entropy. */
function randomToken(byteLength: number): string {
  const bytes = new Uint8Array(byteLength);
  crypto.getRandomValues(bytes);
  return base64UrlEncode(bytes);
}

/** RFC 7636 S256: `code_verifier` is 32 bytes of entropy (well over the
 * spec's own 43-character minimum once base64url-encoded); `code_challenge`
 * is `BASE64URL(SHA256(code_verifier))`. */
export async function generatePkcePair(): Promise<{ codeVerifier: string; codeChallenge: string }> {
  const codeVerifier = randomToken(32);
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(codeVerifier));
  const codeChallenge = base64UrlEncode(new Uint8Array(digest));
  return { codeVerifier, codeChallenge };
}

export function generateState(): string {
  return randomToken(24);
}

export function generateNonce(): string {
  return randomToken(24);
}

/**
 * Constant-time equality over two strings — `admin/middleware.ts`'s
 * pre-#194 Basic-auth gate already established this exact pattern
 * (`digestsMatch`) for the identical reason: `state` is the console's one
 * CSRF defence on this flow (#194's own hard requirement — "state...
 * verified on the callback"), and a naive `===` leaks a timing signal
 * proportional to the matching prefix length.
 */
export function timingSafeEqual(a: string, b: string): boolean {
  if (a.length !== b.length) return false;
  let diff = 0;
  for (let i = 0; i < a.length; i++) {
    diff |= a.charCodeAt(i) ^ b.charCodeAt(i);
  }
  return diff === 0;
}

/**
 * **The state check.** `returnedState` is what the OP's `/login` response
 * echoed back in the redirect URL (attacker-observable, attacker-
 * replayable); `expectedState` is what this same browser's own
 * `vsms_oidc_txn` cookie says it generated at the start of this login
 * attempt. A mismatch — or either being empty, which a broken caller could
 * otherwise pass off as "no check to fail" — is always rejected.
 */
export function verifyState(returnedState: string, expectedState: string): boolean {
  if (returnedState.length === 0 || expectedState.length === 0) return false;
  return timingSafeEqual(returnedState, expectedState);
}

/**
 * **The nonce check**, the OIDC-specific sibling of `verifyState` above —
 * defends the `id_token` itself (replay/substitution across two concurrent
 * login attempts on the same browser) rather than the authorization-code
 * redirect. Same shape, same fail-closed-on-empty rule, checked against the
 * `nonce` claim `sms-gateway`'s own `/token` stamps onto the `id_token`
 * (`authkestra_op`'s real `issue_id_token`, unmodified — see
 * `app/sms-gateway/src/login.rs`'s own doc).
 */
export function verifyNonce(idTokenNonce: string | undefined, expectedNonce: string): boolean {
  if (idTokenNonce === undefined || idTokenNonce.length === 0 || expectedNonce.length === 0) {
    return false;
  }
  return timingSafeEqual(idTokenNonce, expectedNonce);
}

/**
 * A256GCM needs exactly 32 raw bytes. `SMS_CONSOLE_SESSION_SECRET` is an
 * operator-chosen string validated `>= 32` *characters* by `@vsms/env`
 * (not 32 bytes of derived key material) — SHA-256 it down to exactly 32
 * bytes rather than requiring the operator to supply base64-encoded key
 * material directly.
 */
async function sessionKey(secret: string): Promise<Uint8Array> {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(secret));
  return new Uint8Array(digest);
}

async function encryptJson(
  payload: Record<string, unknown>,
  secret: string,
  ttlSeconds: number,
): Promise<string> {
  const key = await sessionKey(secret);
  return new EncryptJWT(payload)
    .setProtectedHeader({ alg: "dir", enc: "A256GCM" })
    .setIssuedAt()
    .setExpirationTime(`${ttlSeconds}s`)
    .encrypt(key);
}

/** `undefined` on any failure — a wrong key, a tampered/truncated/expired
 * cookie, or malformed payload all collapse to "no valid session/txn",
 * never a thrown exception a caller might forget to catch. This is a
 * security boundary: every caller must treat "cannot decrypt" identically
 * to "not logged in" / "no login in progress". */
async function decryptJson<T>(token: string, secret: string): Promise<T | undefined> {
  try {
    const key = await sessionKey(secret);
    const { payload } = await jwtDecrypt(token, key);
    return payload as unknown as T;
  } catch {
    return undefined;
  }
}

export async function encryptTxn(txn: OidcTxn, secret: string): Promise<string> {
  return encryptJson({ ...txn }, secret, TXN_TTL_SECONDS);
}

export async function decryptTxn(token: string, secret: string): Promise<OidcTxn | undefined> {
  return decryptJson<OidcTxn>(token, secret);
}

/** Session TTL matches §5.3's own human refresh-token figure (8h) — the
 * cookie simply stops decrypting (expired JWE `exp`) at the same point
 * `admin`'s own refresh logic would otherwise have to give up anyway, one
 * enforcement point instead of two. */
const SESSION_TTL_SECONDS = 8 * 60 * 60;

export async function encryptSession(session: Session, secret: string): Promise<string> {
  return encryptJson({ ...session }, secret, SESSION_TTL_SECONDS);
}

export async function decryptSession(token: string, secret: string): Promise<Session | undefined> {
  return decryptJson<Session>(token, secret);
}

export const TXN_COOKIE_MAX_AGE_SECONDS = TXN_TTL_SECONDS;
export const SESSION_COOKIE_MAX_AGE_SECONDS = SESSION_TTL_SECONDS;
