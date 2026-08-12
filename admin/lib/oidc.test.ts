// verifyState/verifyNonce are the actual CSRF/replay defence #194's own
// hard requirement names ("state and nonce, both verified on the
// callback") — the Rust side (app/sms-gateway/tests/login_flow_live_postgres.rs)
// proves state round-trips unchanged through /login's own redirect, but
// the *verification* — rejecting a value that doesn't match — is this
// module's job, run in admin/app/api/auth/callback/route.ts. These tests
// are the guard-failure proof for that half: a genuine mismatch must be
// rejected, and — per the house standard — this was verified to actually
// catch a real regression, not merely asserted to pass. See the PR
// description for the exact before/after failure output: `verifyState`
// was temporarily changed to `return returnedState.length > 0 &&
// expectedState.length > 0` (i.e. "both present" instead of "equal"),
// which made `a_mismatched_state_is_rejected` below fail with `expected
// false to be true`; restoring the real `timingSafeEqual` call restored a
// pass.

import { describe, expect, it } from "vitest";
import {
  encryptSession,
  encryptTxn,
  generateNonce,
  generatePkcePair,
  generateState,
  timingSafeEqual,
  verifyNonce,
  verifyState,
} from "./oidc";

describe("verifyState", () => {
  it("accepts a state that matches exactly", () => {
    const state = generateState();
    expect(verifyState(state, state)).toBe(true);
  });

  it("a_mismatched_state_is_rejected", () => {
    const expected = generateState();
    const forged = generateState();
    expect(forged).not.toBe(expected);
    expect(verifyState(forged, expected)).toBe(false);
  });

  it("rejects an empty returned state even against an empty expected state", () => {
    // Neither side of the comparison being empty may ever read as "trivially
    // equal, so allow it" — an empty vsms_oidc_txn cookie (missing/expired)
    // must never be treated as "no check needed".
    expect(verifyState("", "")).toBe(false);
  });

  it("rejects a state one character short of the real one", () => {
    const expected = generateState();
    const almost = expected.slice(0, -1);
    expect(verifyState(almost, expected)).toBe(false);
  });
});

describe("verifyNonce", () => {
  it("accepts a nonce that matches exactly", () => {
    const nonce = generateNonce();
    expect(verifyNonce(nonce, nonce)).toBe(true);
  });

  it("rejects a mismatched nonce", () => {
    const expected = generateNonce();
    const forged = generateNonce();
    expect(verifyNonce(forged, expected)).toBe(false);
  });

  it("rejects an undefined id_token nonce claim", () => {
    expect(verifyNonce(undefined, generateNonce())).toBe(false);
  });
});

describe("timingSafeEqual", () => {
  it("is a real equality check, not a length-only check", () => {
    expect(timingSafeEqual("abc", "abc")).toBe(true);
    expect(timingSafeEqual("abc", "abd")).toBe(false);
    expect(timingSafeEqual("abc", "ab")).toBe(false);
  });
});

describe("generatePkcePair", () => {
  it("the challenge is the base64url(sha256(verifier)) — computed independently", async () => {
    const { codeVerifier, codeChallenge } = await generatePkcePair();
    const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(codeVerifier));
    const expected = btoa(String.fromCharCode(...new Uint8Array(digest)))
      .replaceAll("+", "-")
      .replaceAll("/", "_")
      .replaceAll("=", "");
    expect(codeChallenge).toBe(expected);
  });

  it("two calls produce different verifiers", async () => {
    const a = await generatePkcePair();
    const b = await generatePkcePair();
    expect(a.codeVerifier).not.toBe(b.codeVerifier);
  });
});

describe("txn/session round trip", () => {
  const secret = "a-32-character-or-longer-test-secret!!";

  it("a txn encrypted with one secret decrypts back to the same values", async () => {
    const { decryptTxn } = await import("./oidc");
    const txn = { state: generateState(), nonce: generateNonce(), codeVerifier: "verifier-value" };
    const token = await encryptTxn(txn, secret);
    const decrypted = await decryptTxn(token, secret);
    // toMatchObject, not toEqual: jose's EncryptJWT always adds its own
    // `iat`/`exp` claims to the payload alongside the fields this test
    // actually cares about — real, expected, and not this test's concern.
    expect(decrypted).toMatchObject(txn);
  });

  it("decrypting with the wrong secret fails closed, not with an exception", async () => {
    const { decryptTxn } = await import("./oidc");
    const txn = { state: "s", nonce: "n", codeVerifier: "v" };
    const token = await encryptTxn(txn, secret);
    const decrypted = await decryptTxn(token, "a-completely-different-32-char-secret");
    expect(decrypted).toBeUndefined();
  });

  it("a tampered token fails closed", async () => {
    const { decryptTxn } = await import("./oidc");
    const token = await encryptTxn({ state: "s", nonce: "n", codeVerifier: "v" }, secret);
    const tampered = `${token.slice(0, -4)}zzzz`;
    expect(await decryptTxn(tampered, secret)).toBeUndefined();
  });

  it("a session round-trips its bearer token and claims", async () => {
    const { decryptSession } = await import("./oidc");
    const session = {
      accessToken: "at.value",
      refreshToken: "rt.value",
      accessTokenExpiresAtMs: Date.now() + 900_000,
      subject: "user123",
      email: "ops@example.cm",
      displayName: "Ops User",
      role: "operator",
    };
    const token = await encryptSession(session, secret);
    const decrypted = await decryptSession(token, secret);
    expect(decrypted).toMatchObject(session);
  });
});
