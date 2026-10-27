// One pairing, from a key to an accepted key, as a function of its
// dependencies so a test can drive it without a browser (the screen is
// app/pair/pair-screen.tsx).
//
//   1. Read the key's status. Paired: nothing to do. Ended (expired or revoked,
//      on a clock that is right): the key can never be accepted again, so it is
//      wiped and replaced. Clock (this computer's clock is wrong, so a refusal
//      says nothing about the key): the key is left exactly as it is and the
//      flow stops, "held". It is wiped only when the person asks to pair again
//      (`replace`), because a key the gateway may still accept is not ours to
//      delete on the strength of a wrong clock.
//   2. Make or reuse this browser's non-extractable key.
//   3. Show the QR and wait. The wait asks the gateway, with a sealed read
//      signed by the key just made, whether the phone has vouched for it yet.
//      Before it has, the gateway knows no such key and answers its unsigned
//      401; that is "not yet", never an error, and neither is a dropped
//      connection (the screen only says it cannot reach the gateway). A QR is
//      redrawn before the gateway's five minutes run out under a phone that is
//      still scanning it.
//
// The gateway is the only authority on whether the key is accepted. The wait
// ends on its 200 and on nothing else.

import type { DeviceKey } from "./device-key";
import type { KeyStatus } from "./key-lifecycle";
import {
  base64Url,
  buildPairingQr,
  type PairingQr,
  QR_REFRESH_SECONDS,
  waitForApproval,
} from "./pairing";

export type PairScreen =
  | { kind: "starting" }
  | { kind: "waiting"; qr: PairingQr; label: string; reachable: boolean; publicKey: string }
  | { kind: "paired" }
  /** The clock is wrong: the key was not touched and nothing was started. */
  | { kind: "clock"; skewSeconds: number };

export interface PairFlowDeps {
  status: () => Promise<KeyStatus>;
  /** Forgets the stored key and the runtime built from it. */
  wipeKey: () => Promise<void>;
  /** The stored key, or a fresh one stored now. */
  ensureKey: () => Promise<DeviceKey>;
  /** A sealed read: resolves on the gateway's 200, rejects with `httpStatus`
   *  otherwise. */
  probe: () => Promise<void>;
  nowSeconds: () => number;
  label: () => string;
  sleep?: (ms: number) => Promise<void>;
  intervalMs?: number;
}

function httpStatusOf(error: unknown): number | undefined {
  if (typeof error === "object" && error !== null && "httpStatus" in error) {
    const status = (error as { httpStatus: unknown }).httpStatus;
    return typeof status === "number" ? status : undefined;
  }
  return undefined;
}

export interface PairOptions {
  /** The person asked to pair again: forget the key this browser holds, whatever
   *  its status, and make a new one. */
  replace?: boolean | undefined;
}

/** Runs one pairing. Resolves `"paired"` when the gateway accepted the key,
 *  `"aborted"` when `signal` stopped it and `"held"` when the clock is wrong
 *  and the person has not asked to replace the key. */
export async function runPairing(
  deps: PairFlowDeps,
  show: (screen: PairScreen) => void,
  signal: AbortSignal,
  options: PairOptions = {},
): Promise<"paired" | "aborted" | "held"> {
  show({ kind: "starting" });
  if (options.replace === true) {
    await deps.wipeKey();
  } else {
    const first = await deps.status();
    if (first.kind === "paired") {
      show({ kind: "paired" });
      return "paired";
    }
    if (first.kind === "clock") {
      show({ kind: "clock", skewSeconds: first.skewSeconds });
      return "held";
    }
    if (first.kind === "ended") await deps.wipeKey();
  }
  if (signal.aborted) return "aborted";

  const key = await deps.ensureKey();
  const label = deps.label();
  const publicKey = base64Url(key.publicKey);

  while (!signal.aborted) {
    const qr = await buildPairingQr(key, label, deps.nowSeconds());
    let reachable = true;
    show({ kind: "waiting", qr, label, reachable, publicKey });
    const accepted = await waitForApproval({
      check: async () => {
        try {
          await deps.probe();
          return;
        } catch (error) {
          // A 401 is the gateway saying it does not know the key yet.
          const heard = httpStatusOf(error) === 401;
          if (heard !== reachable) {
            reachable = heard;
            show({ kind: "waiting", qr, label, reachable, publicKey });
          }
          throw error;
        }
      },
      signal,
      expired: () => deps.nowSeconds() - qr.issuedAt >= QR_REFRESH_SECONDS,
      ...(deps.sleep ? { sleep: deps.sleep } : {}),
      ...(deps.intervalMs !== undefined ? { intervalMs: deps.intervalMs } : {}),
    });
    if (accepted) {
      show({ kind: "paired" });
      return "paired";
    }
  }
  return "aborted";
}
