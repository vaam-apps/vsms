"use client";

// This browser's key status, read once and shared: the gate decides what to
// render from it, the shell shows it in the account block and the pairing and
// console screens act on it. A hook with a context rather than a query,
// because the answer is not data the gateway owns: it joins IndexedDB, the
// gateway and the clock (`lib/key-lifecycle.ts`).

import { getConsoleKeyByThumbprint, probeKey } from "@vsms/gateway";
import {
  createContext,
  type ReactNode,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useReducer,
  useRef,
} from "react";
import { indexedDbKeyStore, markPaired } from "@/lib/device-key";
import { onUnauthenticated } from "@/lib/key-events";
import { type KeyStatus, readKeyStatus, shouldRereadOnRefusal } from "@/lib/key-lifecycle";
import { coseThumbprint } from "@/lib/thumbprint";

export type KeyStatusState = KeyStatus | { kind: "checking" };

export interface KeyStatusValue {
  status: KeyStatusState;
  /** The RFC 9679 thumbprint of this browser's key, once known. */
  thumbprint: string | undefined;
  /** Reads the status again (after pairing, unpairing or a refusal). Resolves
   *  once the new status is in place, so a caller that is about to navigate can
   *  wait for it: the gate would otherwise act on the stale one. */
  refresh: () => Promise<void>;
}

const KeyStatusContext = createContext<KeyStatusValue | undefined>(undefined);

interface Read {
  status: KeyStatus;
  thumbprint: string | undefined;
}

async function thumbprintOfStored(): Promise<string | undefined> {
  try {
    const stored = await indexedDbKeyStore.get();
    if (stored === undefined) return undefined;
    return await coseThumbprint(
      new Uint8Array(await crypto.subtle.exportKey("raw", stored.pair.publicKey)),
    );
  } catch {
    return undefined;
  }
}

/** The status, with the real IndexedDB, gateway and clock behind it. */
async function readNow(): Promise<Read> {
  const status = await readStatus();
  return { status, thumbprint: await thumbprintOfStored() };
}

function readStatus(): Promise<KeyStatus> {
  return readKeyStatus({
    load: () => indexedDbKeyStore.get(),
    probe: probeKey,
    lookup: getConsoleKeyByThumbprint,
    thumbprint: async (stored) =>
      coseThumbprint(new Uint8Array(await crypto.subtle.exportKey("raw", stored.pair.publicKey))),
    remember: (meta) => markPaired(meta),
    now: Date.now,
  });
}

interface State {
  status: KeyStatusState;
  thumbprint: string | undefined;
}

function reduce(_: State, read: Read): State {
  return read;
}

export function KeyStatusProvider({ children }: { children: ReactNode }) {
  const [{ status, thumbprint }, dispatch] = useReducer(reduce, {
    status: { kind: "checking" },
    thumbprint: undefined,
  } as State);
  // Only the newest read counts: a slow answer must not overwrite a later one.
  const generation = useRef(0);
  const latest = useRef<KeyStatusState["kind"]>("checking");
  latest.current = status.kind;
  const reading = useRef(false);

  const refresh = useCallback((): Promise<void> => {
    const mine = ++generation.current;
    reading.current = true;
    return readNow()
      .then((next) => {
        if (mine === generation.current) dispatch(next);
      })
      .finally(() => {
        if (mine === generation.current) reading.current = false;
      });
  }, []);

  useEffect(() => {
    void refresh();
    // A 401 from a screen's data call means the gateway just stopped accepting a
    // key it accepted: read the status again (`shouldRereadOnRefusal` says when,
    // and why it is not always).
    const unsubscribe = onUnauthenticated(() => {
      if (shouldRereadOnRefusal(latest.current, reading.current)) void refresh();
    });
    const onVisible = () => {
      if (document.visibilityState === "visible") void refresh();
    };
    document.addEventListener("visibilitychange", onVisible);
    return () => {
      unsubscribe();
      document.removeEventListener("visibilitychange", onVisible);
    };
  }, [refresh]);

  const value = useMemo(() => ({ status, thumbprint, refresh }), [status, thumbprint, refresh]);
  return <KeyStatusContext.Provider value={value}>{children}</KeyStatusContext.Provider>;
}

export function useKeyStatus(): KeyStatusValue {
  const value = useContext(KeyStatusContext);
  if (value === undefined) throw new Error("useKeyStatus is used outside the console gate");
  return value;
}
