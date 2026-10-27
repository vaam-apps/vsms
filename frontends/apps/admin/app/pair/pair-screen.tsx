"use client";

// The pairing screen (docs/design/console-pairing.md): drives one pairing
// (`lib/pair-flow.ts`) and hands the result to `PairView`. It makes the key,
// shows the QR and waits; the gateway's 200 to a sealed read is the only thing
// that ends the wait. Before VA-4 (vaam relaying the QR) the operator pairs with
// `sms-gateway pair-console-key`; this screen cannot tell the two apart and does
// not need to.

import { getConsoleKeyByThumbprint, probeKey } from "@vsms/gateway";
import { useRouter } from "next/navigation";
import { useEffect, useReducer } from "react";
import { clockSkewSeconds, isSkewed } from "@/lib/clock";
import {
  deleteDeviceKey,
  indexedDbKeyStore,
  loadOrCreateDeviceKey,
  markPaired,
} from "@/lib/device-key";
import { readKeyStatus } from "@/lib/key-lifecycle";
import { runPairing, type PairScreen as Screen } from "@/lib/pair-flow";
import { browserLabel } from "@/lib/pairing";
import { forgetSealer } from "@/lib/sealer";
import { coseThumbprint } from "@/lib/thumbprint";
import { useKeyStatus } from "../key-status";
import { PairView } from "./components/pair-view";

interface State {
  screen: Screen;
  failure: string | undefined;
  replacing: boolean;
  skewSeconds: number | undefined;
}

type Action =
  | { type: "screen"; screen: Screen }
  | { type: "failed"; message: string }
  | { type: "replacing" }
  | { type: "skew"; seconds: number | undefined };

function reduce(state: State, action: Action): State {
  switch (action.type) {
    case "screen":
      return { ...state, screen: action.screen, failure: undefined };
    case "failed":
      return { ...state, failure: action.message };
    case "replacing":
      return { ...state, replacing: true };
    case "skew":
      return { ...state, skewSeconds: action.seconds };
  }
}

const INITIAL: State = {
  screen: { kind: "starting" },
  failure: undefined,
  replacing: false,
  skewSeconds: undefined,
};

async function measureSkew(): Promise<number | undefined> {
  try {
    const response = await fetch("/api/health", { cache: "no-store", credentials: "omit" });
    return clockSkewSeconds(response.headers.get("date"), Date.now());
  } catch {
    return undefined;
  }
}

export function PairScreen() {
  const router = useRouter();
  const { refresh } = useKeyStatus();
  const [state, dispatch] = useReducer(reduce, INITIAL);

  useEffect(() => {
    const abort = new AbortController();
    void measureSkew().then((seconds) => dispatch({ type: "skew", seconds }));

    const status = () =>
      readKeyStatus({
        load: () => indexedDbKeyStore.get(),
        probe: probeKey,
        lookup: getConsoleKeyByThumbprint,
        thumbprint: async (stored) =>
          coseThumbprint(
            new Uint8Array(await crypto.subtle.exportKey("raw", stored.pair.publicKey)),
          ),
        remember: (meta) => markPaired(meta),
        now: Date.now,
      });

    runPairing(
      {
        status: async () => {
          const read = await status();
          if (read.kind === "ended") dispatch({ type: "replacing" });
          return read;
        },
        wipeKey: async () => {
          await deleteDeviceKey();
          forgetSealer();
        },
        ensureKey: async () => {
          const key = await loadOrCreateDeviceKey();
          forgetSealer();
          return key;
        },
        probe: probeKey,
        nowSeconds: () => Math.floor(Date.now() / 1000),
        label: () => browserLabel(),
      },
      (screen) => dispatch({ type: "screen", screen }),
      abort.signal,
    )
      .then(async (result) => {
        if (result !== "paired" || abort.signal.aborted) return;
        // Learn the key's id and expiry from the gateway, then open the console.
        await status();
        // Wait for the shared status to say "paired" before leaving: the gate
        // would otherwise see the stale one at "/" and send the person back.
        await refresh();
        router.replace("/");
      })
      .catch((error: unknown) => {
        if (abort.signal.aborted) return;
        dispatch({
          type: "failed",
          message:
            error instanceof Error
              ? `Could not prepare a key: ${error.message}`
              : "Could not prepare a key.",
        });
      });

    return () => abort.abort();
  }, [refresh, router]);

  const skewed = isSkewed(state.skewSeconds);
  return (
    <PairView
      screen={state.screen}
      failure={state.failure}
      replacing={state.replacing}
      clockSkewMinutes={
        skewed && state.skewSeconds !== undefined ? Math.round(state.skewSeconds / 60) : undefined
      }
    />
  );
}
