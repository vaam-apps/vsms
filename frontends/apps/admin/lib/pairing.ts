// Pairing this console with a vaam admin phone (docs/design/console-pairing.md;
// the screen is app/pair/page.tsx).
//
// The console shows a QR holding the call the phone will relay to vsms, in vaam's
// own admin-pairing format so one scanner reads both:
//
//   {"v":1,"k":<65-byte uncompressed P-256 point, base64url>,
//    "t":<unix seconds>,"l":<label, at most 40 characters>,
//    "s":<ES256 r‖s over "vsms.console-pair.v1\n" + k + "\n" + t, base64url>}
//
// `s` proves the browser holds the key before anyone is asked for a fingerprint,
// and `registerConsoleKey` checks it again (`backends/crates/sms-api/src/
// console_keys.rs`, `possession_message`): the phone passes `k` as `publicKey`,
// `t` as `issuedAt` and `s` as `possessionProof`. The label is the browser's own
// claim and is NOT signed (the gateway's message has no slot for it): the phone
// shows it as a claim, and the operator confirms by what they see on this
// screen. The tag is what keeps the signature from standing for anything else,
// including vaam's own `vaam.admin-pair.v1`.

import type { DeviceKey } from "./device-key";

/** How long the gateway accepts a QR, either side of its own clock. */
export const QR_LIFETIME_SECONDS = 300;
/** A QR is drawn afresh this long after it was made, before it can expire under
 *  a phone that is still scanning it. */
export const QR_REFRESH_SECONDS = 240;
/** How often the page asks whether the phone has approved. */
export const POLL_INTERVAL_MS = 2000;
/** The longest label a QR carries (vaam's own limit for a browser's label). */
export const LABEL_MAX_CHARS = 40;
/** A key lasts this long from the moment the gateway accepts it
 *  (`ConsoleKey.expiresAt`, owner ruling R3). The gateway is the authority; this
 *  is for the words on the screen only. */
export const KEY_LIFETIME_DAYS = 30;

/** The domain tag of the possession proof (`PROOF_TAG` in console_keys.rs). */
const POSSESSION_TAG = "vsms.console-pair.v1";

/** base64url without padding. */
export function base64Url(bytes: Uint8Array): string {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

/** The bytes the browser signs, as `possession_message` in console_keys.rs
 *  builds them: the tag, the key as it is spelled in the QR, and the time, one
 *  per line.
 *
 *  ```ts ignore
 *  possessionMessage("a2V5", 1_700_000_000);
 *  // "vsms.console-pair.v1\na2V5\n1700000000", as UTF-8
 *  ```
 */
export function possessionMessage(publicKey: string, issuedAt: number): Uint8Array {
  return new TextEncoder().encode(`${POSSESSION_TAG}\n${publicKey}\n${issuedAt}`);
}

/** A label the QR may carry: printable ASCII (the QR carries bytes, not code
 *  points), single-spaced, trimmed, at most {@link LABEL_MAX_CHARS}. */
export function cleanLabel(raw: string): string {
  const printable = raw
    .replace(/[^\x20-\x7e]/g, " ")
    .replace(/\s+/g, " ")
    .trim()
    .slice(0, LABEL_MAX_CHARS)
    .trim();
  return printable === "" ? "Browser" : printable;
}

interface NavigatorWithBrands {
  userAgentData?: { brands?: { brand: string }[]; platform?: string };
  userAgent?: string;
  platform?: string;
}

/** What this browser calls itself, e.g. "Chrome on macOS". A claim, nothing
 *  more: anyone can change it, which is why the phone says so. */
export function browserLabel(nav: NavigatorWithBrands = globalThis.navigator): string {
  const brands = nav.userAgentData?.brands?.map((entry) => entry.brand) ?? [];
  const brand =
    brands.find((name) => !/not.?a.?brand|chromium/i.test(name)) ??
    brands[0] ??
    userAgentBrand(nav.userAgent);
  const platform = nav.userAgentData?.platform || nav.platform || "";
  return cleanLabel([brand, platform].filter(Boolean).join(" on "));
}

function userAgentBrand(userAgent = ""): string {
  const known: [RegExp, string][] = [
    [/edg\//i, "Edge"],
    [/firefox\//i, "Firefox"],
    [/chrome\//i, "Chrome"],
    [/safari\//i, "Safari"],
  ];
  return known.find(([pattern]) => pattern.test(userAgent))?.[1] ?? "Browser";
}

/** The QR the page draws. */
export interface PairingQr {
  /** The text to encode. */
  text: string;
  /** When it was made, in Unix seconds. */
  issuedAt: number;
}

/** Builds the QR for `key`. `issuedAt` is Unix seconds on THIS computer's clock:
 *  the gateway checks it within {@link QR_LIFETIME_SECONDS}, and so does every
 *  sealed request that follows, so a clock that is further out than that cannot
 *  use the console at all (the pairing page says so, `lib/clock.ts`). */
export async function buildPairingQr(
  key: Pick<DeviceKey, "publicKey" | "sign">,
  label: string,
  issuedAt: number,
): Promise<PairingQr> {
  const k = base64Url(key.publicKey);
  const t = Math.floor(issuedAt);
  const s = base64Url(await key.sign(possessionMessage(k, t)));
  return { text: JSON.stringify({ v: 1, k, t, l: cleanLabel(label), s }), issuedAt: t };
}

/** Asks `check` every `intervalMs` until it succeeds, the signal aborts, or
 *  `expired` says the QR on screen has outlived its refresh time. Each ask that
 *  fails is just "not yet": before approval the gateway answers a key it has not
 *  heard of with its unsigned 401, a hint and never proof, and a flaky network
 *  must not end a pairing the phone is finishing. `fatal` ends the wait instead
 *  (rethrown): the key is spent and the page must make a new one.
 *
 *  Resolves `true` when `check` succeeded, `false` when stopped. */
export async function waitForApproval(options: {
  check: () => Promise<void>;
  intervalMs?: number;
  signal?: AbortSignal;
  expired?: () => boolean;
  fatal?: (error: unknown) => boolean;
  sleep?: (ms: number) => Promise<void>;
}): Promise<boolean> {
  const {
    check,
    intervalMs = POLL_INTERVAL_MS,
    signal,
    expired = () => false,
    fatal = () => false,
    sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms)),
  } = options;
  while (!signal?.aborted && !expired()) {
    try {
      await check();
      return true;
    } catch (error) {
      if (fatal(error)) throw error;
      // Not yet.
    }
    await sleep(intervalMs);
  }
  return false;
}
