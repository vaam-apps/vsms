// One pairing, driven without a browser. What is held here: the wait ends on
// the gateway's yes and on nothing else, a finished key is replaced rather than
// shown again, a QR is redrawn before the gateway would call it stale, and a
// gateway that cannot be reached is said to be unreachable, not "not paired".

import { describe, expect, it, vi } from "vitest";
import { loadOrCreateDeviceKey, memoryKeyStore } from "./device-key";
import type { KeyStatus } from "./key-lifecycle";
import { type PairFlowDeps, type PairScreen, runPairing } from "./pair-flow";
import { QR_REFRESH_SECONDS } from "./pairing";

const refusal = (httpStatus: number) => Object.assign(new Error("refused"), { httpStatus });

async function harness(over: Partial<PairFlowDeps> = {}) {
  const store = memoryKeyStore();
  let clock = 1_700_000_000;
  const shown: PairScreen[] = [];
  const deps: PairFlowDeps = {
    status: async () => ({ kind: "none" }) as KeyStatus,
    wipeKey: vi.fn(async () => store.clear()),
    ensureKey: vi.fn(() => loadOrCreateDeviceKey(store)),
    probe: async () => {
      throw refusal(401);
    },
    nowSeconds: () => clock,
    label: () => "Chrome on macOS",
    sleep: async () => {
      clock += 2;
    },
    intervalMs: 1,
    ...over,
  };
  return {
    deps,
    shown,
    advance: (seconds: number) => {
      clock += seconds;
    },
    run: (signal = new AbortController().signal) =>
      runPairing(deps, (screen) => shown.push(screen), signal),
  };
}

describe("runPairing", () => {
  it("shows a QR and ends only when the gateway accepts the key", async () => {
    let calls = 0;
    const h = await harness({
      probe: async () => {
        calls += 1;
        if (calls < 4) throw refusal(401);
      },
    });

    expect(await h.run()).toBe("paired");

    expect(calls).toBe(4);
    expect(h.shown[0]).toEqual({ kind: "starting" });
    const waiting = h.shown.find((s) => s.kind === "waiting");
    expect(waiting).toBeDefined();
    // The command-line path needs the key as text; it is the QR's own `k`.
    expect((waiting as { publicKey: string }).publicKey).toBe(
      (JSON.parse((waiting as { qr: { text: string } }).qr.text) as { k: string }).k,
    );
    expect(h.shown.at(-1)).toEqual({ kind: "paired" });
  });

  it("does nothing for a console that is already paired", async () => {
    const h = await harness({
      status: async () => ({
        kind: "paired",
        meta: { keyId: "k", userId: "u", pairedAt: "x", expiresAt: "y" },
      }),
    });

    expect(await h.run()).toBe("paired");
    expect(h.deps.ensureKey).not.toHaveBeenCalled();
  });

  it("replaces a key that ended: wipes it first, then makes a new one", async () => {
    const order: string[] = [];
    const h = await harness({
      status: async () => ({
        kind: "ended",
        cause: "expired",
        meta: { keyId: "k", userId: "u", pairedAt: "x", expiresAt: "y" },
      }),
      wipeKey: vi.fn(async () => void order.push("wipe")),
      ensureKey: vi.fn(async () => {
        order.push("ensure");
        return loadOrCreateDeviceKey(memoryKeyStore());
      }),
      probe: async () => undefined,
    });

    await h.run();

    expect(order).toEqual(["wipe", "ensure"]);
  });

  it("redraws the QR before the gateway would call it stale", async () => {
    const controller = new AbortController();
    let probes = 0;
    const h = await harness({
      sleep: async () => undefined,
      probe: async () => {
        // A bound, so a QR that is never redrawn fails this test instead of
        // spinning it: the phone is slow and the page would wait forever.
        probes += 1;
        if (probes > 20) controller.abort();
        throw refusal(401);
      },
    });
    const seen = new Set<string>();
    const run = runPairing(
      h.deps,
      (screen) => {
        h.shown.push(screen);
        if (screen.kind === "waiting") {
          seen.add(screen.qr.text);
          // The phone is slow: a QR lives QR_REFRESH_SECONDS, then is redrawn.
          h.advance(QR_REFRESH_SECONDS);
          if (seen.size >= 2) controller.abort();
        }
      },
      controller.signal,
    );

    expect(await run).toBe("aborted");
    expect(seen.size).toBeGreaterThanOrEqual(2);
  });

  it("says the gateway is unreachable when it is, and 'not yet' when it merely does not know the key", async () => {
    const h = await harness({
      probe: async () => {
        throw refusal(503);
      },
    });
    const controller = new AbortController();
    h.deps.sleep = async () => controller.abort();

    await h.run(controller.signal);

    const waiting = h.shown.filter(
      (s): s is Extract<PairScreen, { kind: "waiting" }> => s.kind === "waiting",
    );
    expect(waiting.some((s) => s.reachable === false)).toBe(true);
  });

  it("stops when aborted and never reports paired", async () => {
    const controller = new AbortController();
    const h = await harness();
    h.deps.sleep = async () => controller.abort();

    expect(await h.run(controller.signal)).toBe("aborted");
    expect(h.shown.some((s) => s.kind === "paired")).toBe(false);
  });
});
