import { createHash, createHmac, timingSafeEqual } from "node:crypto";

/**
 * ============================================================================
 * PROVISIONAL SEAM — replace this file, and only this file, when #41 lands.
 * ============================================================================
 *
 * vsms has never shipped outbound webhook signing. #41 tracks it and it is
 * not implemented. What follows is this receiver's best-effort read of
 * `docs/architecture.md` §4.4, which documents an *intended* design, not a
 * verified one — nothing in this repo has generated a signature with it,
 * live, ever. Treat every detail below as "vsms's current design intent,"
 * subject to change the moment #41 actually lands and gets tested against a
 * real sender.
 *
 * §4.4's documented shape:
 *
 *   X-Sms-Event:     message.delivered
 *   X-Sms-Event-Id:  <sourceEventId>
 *   X-Sms-Timestamp: <unix seconds>
 *   X-Sms-Signature: v1=<hex>[,v1=<hex during rotation>]
 *
 *   signing string = "v1\n{timestamp}\n{eventId}\n{sha256(body)}"
 *   key            = WebhookEndpoint.secret (or .prevSecret during rotation)
 *
 * Two things the doc does NOT say, that this implementation had to decide:
 *
 * 1. The MAC algorithm. §4.4 shows "v1=<hex>" and a "v1 \n ..." signing
 *    string but never names the algorithm. HMAC-SHA256 is the obvious,
 *    Stripe-style reading of that shape (versioned scheme prefix + hex
 *    digest), and is what's implemented here — but it is a guess filling a
 *    real gap, not a documented fact.
 * 2. Replay/freshness tolerance on `X-Sms-Timestamp`. §4.4 doesn't specify a
 *    tolerance window, so none is enforced here beyond requiring the header
 *    to be present (it's folded into the signed string, so a tampered
 *    timestamp already fails verification). A real deployment will likely
 *    want a bounded window once #41 defines one; this receiver's own
 *    dedupe (see store.ts) only protects against replaying an event this
 *    receiver has already *successfully* processed, not a stolen-signature
 *    replay of a not-yet-seen event.
 *
 * Everything callers need is this one function: `verifySignature`. Nothing
 * else in this example inspects a header or does crypto — when #41 settles
 * the real scheme, this file is the entire diff.
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
