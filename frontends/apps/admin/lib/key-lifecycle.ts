// Where this browser's key stands, read from the three places that know:
// IndexedDB (has this browser a key, and did the gateway ever accept it), the
// gateway (does it accept it now), and the clock (has the key's 30 days run out).
//
// The gateway cannot tell a key it never knew from one it has finished with
// (expired, revoked): both are its unsigned 401. This browser can, because it
// remembers whether the gateway ever said yes. That memory is what lets the
// console say "this console was unpaired, pair it again" instead of showing a QR
// to a person who thinks they are still signed in.

import type { PairedMeta, StoredKey } from "./device-key";

export type KeyStatus =
  /** This browser has no key. */
  | { kind: "none" }
  /** It has a key the gateway has not accepted (yet): a pairing in progress. */
  | { kind: "pending" }
  /** The gateway accepts the key. */
  | { kind: "paired"; meta: PairedMeta }
  /** The key was accepted once and is accepted no more: it expired or was
   *  revoked. It can never be paired again; pairing makes a new one. */
  | { kind: "ended"; meta: PairedMeta; cause: "expired" | "refused" }
  /** The gateway could not be reached, so nothing is known. */
  | { kind: "unreachable" };

export interface StatusDeps {
  load: () => Promise<StoredKey | undefined>;
  /** Resolves when the gateway accepts the key; rejects with an error carrying
   *  `httpStatus` otherwise. */
  probe: () => Promise<void>;
  /** The gateway's row for this key. */
  lookup: (thumbprint: string) => Promise<{
    id: string;
    userId: string;
    pairedAt: string;
    expiresAt: string;
    revokedAt?: string | undefined;
  } | null>;
  thumbprint: (stored: StoredKey) => Promise<string>;
  remember: (meta: PairedMeta) => Promise<void>;
  now: () => number;
}

function httpStatusOf(error: unknown): number | undefined {
  if (typeof error === "object" && error !== null && "httpStatus" in error) {
    const status = (error as { httpStatus: unknown }).httpStatus;
    return typeof status === "number" ? status : undefined;
  }
  return undefined;
}

/** Whether `meta`'s key has run out of days on this computer's clock. */
export function isExpired(meta: PairedMeta, now: number): boolean {
  return Date.parse(meta.expiresAt) <= now;
}

/** Whole days until `meta`'s key expires, rounded up, never below zero. */
export function daysLeft(meta: PairedMeta, now: number): number {
  const remaining = Date.parse(meta.expiresAt) - now;
  return remaining <= 0 ? 0 : Math.ceil(remaining / 86_400_000);
}

/** Reads the key's status. Never throws: whatever it cannot read is
 *  `unreachable`. */
export async function readKeyStatus(deps: StatusDeps): Promise<KeyStatus> {
  let stored: StoredKey | undefined;
  try {
    stored = await deps.load();
  } catch {
    // IndexedDB blocked or unavailable: this browser cannot hold a key at all.
    return { kind: "none" };
  }
  if (stored === undefined) return { kind: "none" };

  // A key past its 30 days is finished whatever the gateway would say, and
  // asking only spends a request: the gateway treats it as no key.
  if (stored.paired !== undefined && isExpired(stored.paired, deps.now())) {
    return { kind: "ended", meta: stored.paired, cause: "expired" };
  }

  try {
    await deps.probe();
  } catch (error) {
    if (httpStatusOf(error) === 401) {
      return stored.paired === undefined
        ? { kind: "pending" }
        : { kind: "ended", meta: stored.paired, cause: "refused" };
    }
    return { kind: "unreachable" };
  }

  if (stored.paired !== undefined) return { kind: "paired", meta: stored.paired };

  // First acceptance: learn the key's id and expiry from the gateway's row, and
  // keep them. The gateway is the authority on both.
  try {
    const row = await deps.lookup(await deps.thumbprint(stored));
    if (row !== null) {
      const meta: PairedMeta = {
        keyId: row.id,
        userId: row.userId,
        pairedAt: row.pairedAt,
        expiresAt: row.expiresAt,
      };
      await deps.remember(meta);
      return { kind: "paired", meta };
    }
  } catch {
    // Accepted but unreadable: fall through to the placeholder below.
  }
  // The gateway accepts the key but showed no row to this user (a policy that
  // hides it, or a blip). Count the key as paired for a full lifetime from now,
  // which can only be too generous by the time it has already lived; the next
  // read of the row corrects it. An empty `keyId` means "cannot revoke from
  // here", which the lifecycle screen says.
  const meta: PairedMeta = {
    keyId: "",
    userId: "",
    pairedAt: new Date(deps.now()).toISOString(),
    expiresAt: new Date(deps.now() + 30 * 86_400_000).toISOString(),
  };
  return { kind: "paired", meta };
}

/** Whether a 401 from a screen's data call should make the console read its key
 *  status again.
 *
 *  Only when the status is `paired`: a 401 then means the gateway just stopped
 *  accepting a key it accepted. In every other status the 401 is the gateway
 *  saying what the status already says (the pairing page's probe is refused
 *  every two seconds, by design), and reading again on each would be a loop
 *  that asks the gateway about itself. And not while a read is in flight: the
 *  refusal in hand is that read's own probe. */
export function shouldRereadOnRefusal(statusKind: string, readInFlight: boolean): boolean {
  return statusKind === "paired" && !readInFlight;
}
