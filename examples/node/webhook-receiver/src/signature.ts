import { createHash, createHmac, timingSafeEqual } from "node:crypto";

/**
 * ============================================================================
 * PARTIALLY-PROVISIONAL SEAM — replace this file, and only this file, when
 * #41 lands. Read the distinction below before assuming more of this is a
 * guess than actually is.
 * ============================================================================
 *
 * vsms has never shipped outbound webhook signing — #41 tracks it and it is
 * not implemented, so nothing here has generated a signature that a real
 * vsms verified, or verified one a real vsms generated. But
 * `docs/architecture.md` §4.4 is NOT vague about the shape: it specifies,
 * literally, the four header names, the exact rotation semantics (two
 * `v1=` values allowed, accept if either verifies), and the exact
 * four-part signing string. This implementation is a transcription of
 * that spec, not a guess, for everything except one thing (see below).
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
 * `rotateWebhookSecret` moves `secret` → `prevSecret`, generates a new
 * `secret`, and — per §4.4 — "a job clears `prevSecret` after 24 hours."
 * This receiver has no such job to sync against (there is no live vsms to
 * sync from), so it's handed both secrets directly at startup; a real
 * deployment needs to keep `prevSecret` around for that same 24-hour
 * window and drop it after, matching whatever vsms itself does server-side.
 *
 * THE ONE GENUINE GUESS: the MAC algorithm. §4.4 shows the "v1=<hex>"
 * wrapper and the four-line signing string but never names the primitive
 * that turns that string into the hex digest. HMAC-SHA256 is this
 * implementation's reading of that shape (a versioned-scheme prefix plus a
 * hex digest is the obvious, Stripe-style convention it's modelled on) —
 * that substitution, and only that one, is unverified against anything
 * upstream.
 *
 * SEPARATELY, NOT FILLING A GAP IN §4.4 — §4.4 says nothing about this
 * either way, so this isn't an inferred reading of an ambiguous spec, it's
 * an explicit scope decision by this example: no bounded freshness/replay
 * window is enforced on `X-Sms-Timestamp`. It is folded into the signed
 * bytes, so a *tampered* timestamp already fails verification (proven by
 * the emitter's forged-signature case) — but a byte-for-byte replay of a
 * previously-valid, correctly-signed request, no matter how old its
 * timestamp, verifies successfully here. The only thing that stops a
 * replay of an *already-processed* event from being acted on twice is
 * `store.ts`'s dedupe (keyed on `X-Sms-Event-Id`, per §4.4's own
 * "receivers need a dedupe key" line) — not this file. A stale-but-never-
 * before-seen forged timestamp on an otherwise-valid signature (impossible
 * without the secret, but worth being explicit about) would still verify;
 * this is demonstrated live by the emitter's "timestamp freshness" check.
 *
 * Everything callers need is this one function: `verifySignature`. Nothing
 * else in this example inspects a header or does crypto — when #41 settles
 * the MAC primitive for real, this file is the entire diff.
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
 *  `admin/middleware.ts`'s `digestsMatch` — that one hand-rolls XOR because
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
