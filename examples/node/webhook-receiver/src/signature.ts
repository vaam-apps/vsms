import { createHash, createHmac, timingSafeEqual } from "node:crypto";

/**
 * ============================================================================
 * #41 LANDED — this file's algorithm is confirmed, not a guess any more.
 * ============================================================================
 *
 * This file was written against `docs/architecture.md` §4.4's prose
 * *before* #41 (outbound webhook signing) existed, with one deliberate,
 * clearly-flagged gap: §4.4 specifies the four header names, the exact
 * rotation semantics, and the exact four-part signing string, but never
 * names the MAC primitive. HMAC-SHA256 was this file's own reading of
 * that shape — a guess, even if an obvious one.
 *
 * #41 shipped `backends/crates/sms-webhook`, the real Rust implementation, and it
 * is HMAC-SHA256 — this file's guess was correct. That is no longer
 * something this comment merely asserts: `cross-language-vectors.test.ts`
 * in this same directory loads
 * `backends/crates/sms-webhook/tests/fixtures/cross_language_vectors.json` — a
 * fixture whose `signatureHeader` values were computed by neither this
 * file's code nor the Rust crate's, but independently, with `openssl dgst
 * -sha256 -hmac` — and asserts `verifySignature` below agrees with every
 * one of them, `expectVerifies: false` cases included. Run it:
 * `pnpm test` (this package's own `package.json` script) or
 * `node --test`.
 *
 * §4.4's documented shape, quoted exactly:
 *
 *   X-Sms-Event:     message.delivered
 *   X-Sms-Event-Id:  <sourceEventId>
 *   X-Sms-Timestamp: <unix seconds>
 *   X-Sms-Signature: v1=<hex>,v1=<hex during rotation>
 *
 *   signing string = v1 \n {timestamp} \n {eventId} \n {sha256(body)}
 *   key            = WebhookEndpoint.secret
 *
 * "Two `v1=` values during rotation, oldest last; receivers accept if any
 * verifies, which makes rotation a non-event for them." — during a
 * rotation, `key` is `.prevSecret` for the older of the two values.
 * `rotateWebhookSecret` (`backends/crates/sms-api/src/procedures.rs`, #41) moves
 * `secret` → `prevSecret` and generates a fresh `secret`
 * (`sms_webhook::generate_secret()`, `whsec_<64 hex chars>`). §4.4 also
 * says "a job clears `prevSecret` after 24 hours" — that job
 * (§7.5's `cleanup_secrets`) is explicitly out of #41's own scope (see
 * `rotate_webhook_secret`'s doc comment in `procedures.rs`), so this
 * receiver's stance is unchanged: it has no such job to sync against,
 * it's handed both secrets directly at startup, and a real deployment
 * needs to keep `prevSecret` around for that same 24-hour window and drop
 * it after, matching whatever vsms itself does server-side once that job
 * exists.
 *
 * STILL A DELIBERATE SCOPE DECISION, NOT A GAP IN §4.4 OR IN #41: no
 * bounded freshness/replay window is enforced on `X-Sms-Timestamp` here.
 * It is folded into the signed bytes, so a *tampered* timestamp already
 * fails verification (proven by the emitter's forged-signature case) —
 * but a byte-for-byte replay of a previously-valid, correctly-signed
 * request, no matter how old its timestamp, verifies successfully here.
 * `sms-webhook` itself *does* now ship a composable freshness check
 * (`is_timestamp_fresh`) for a caller that wants one — a receiver in this
 * language could call the equivalent check before or after
 * `verifySignature`, this example simply doesn't. The only thing that
 * stops a replay of an *already-processed* event from being acted on
 * twice here is `store.ts`'s dedupe (keyed on `X-Sms-Event-Id`, per
 * §4.4's own "receivers need a dedupe key" line) — not this file. A
 * stale-but-never-before-seen forged timestamp on an otherwise-valid
 * signature (impossible without the secret, but worth being explicit
 * about) would still verify; this is demonstrated live by the emitter's
 * "timestamp freshness" check.
 *
 * Everything callers need is this one function: `verifySignature`.
 * Nothing else in this example inspects a header or does crypto.
 */

export interface VerifyInput {
  rawBody: Buffer;
  timestamp: string | undefined;
  eventId: string | undefined;
  signatureHeader: string | undefined;
  /** Accepts current AND previous secret, per §4.4's rotation-overlap design. */
  secrets: readonly string[];
}

export type VerifyResult = { ok: true } | { ok: false; reason: string };

function sha256Hex(data: Buffer): string {
  return createHash("sha256").update(data).digest("hex");
}

function signingString(timestamp: string, eventId: string, rawBody: Buffer): string {
  return `v1\n${timestamp}\n${eventId}\n${sha256Hex(rawBody)}`;
}

/** Exported so the local emitter (a stand-in sender) computes signatures the
 *  same way a real vsms presumably would — one implementation, not two
 *  copies that can silently drift apart. */
export function computeSignature(
  secret: string,
  timestamp: string,
  eventId: string,
  rawBody: Buffer,
): string {
  return createHmac("sha256", secret)
    .update(signingString(timestamp, eventId, rawBody))
    .digest("hex");
}

const HEX_DIGEST = /^[0-9a-f]+$/i;

/** Constant-time compare of two hex digests, mirroring the pattern in
 *  `frontends/apps/admin/middleware.ts`'s `digestsMatch` — that one hand-rolls XOR because
 *  it runs on the Edge runtime with no `Buffer`; this one has full Node and
 *  uses `crypto.timingSafeEqual` directly. */
function digestsMatch(candidateHex: string, expectedHex: string): boolean {
  if (!HEX_DIGEST.test(candidateHex) || candidateHex.length !== expectedHex.length) return false;
  const candidate = Buffer.from(candidateHex, "hex");
  const expected = Buffer.from(expectedHex, "hex");
  return candidate.length === expected.length && timingSafeEqual(candidate, expected);
}

/**
 * Verifies `X-Sms-Signature` against every secret supplied (current, then
 * previous), and against every `v1=` value present in the header (§4.4 says
 * the header can carry two during a rotation window). Accepts if ANY
 * secret matches ANY presented value — that's what makes rotation a
 * non-event for the receiver.
 */
export function verifySignature(input: VerifyInput): VerifyResult {
  const { rawBody, timestamp, eventId, signatureHeader, secrets } = input;

  if (!timestamp) return { ok: false, reason: "missing X-Sms-Timestamp" };
  if (!eventId) return { ok: false, reason: "missing X-Sms-Event-Id" };
  if (!signatureHeader) return { ok: false, reason: "missing X-Sms-Signature" };

  const presented = signatureHeader
    .split(",")
    .map((entry) => entry.trim())
    .filter((entry) => entry.startsWith("v1="))
    .map((entry) => entry.slice("v1=".length));

  if (presented.length === 0) {
    return { ok: false, reason: "X-Sms-Signature carried no v1= value" };
  }

  const usableSecrets = secrets.filter((secret): secret is string => secret.length > 0);
  for (const secret of usableSecrets) {
    const expected = computeSignature(secret, timestamp, eventId, rawBody);
    for (const candidate of presented) {
      if (digestsMatch(candidate, expected)) {
        return { ok: true };
      }
    }
  }

  return { ok: false, reason: "no presented v1= value matched current or previous secret" };
}
