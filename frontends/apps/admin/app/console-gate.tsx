"use client";

// The console's only gate, and it is not a security boundary: the gateway is.
// Whatever this component renders, no data appears without the browser's key
// signing for it, and a key the gateway does not accept gets the gateway's
// refusal for every call. What this decides is what a person sees: the console,
// or the way to pair it.
//
// `/pair` is the one route that renders without a paired key. Everything else
// waits for the status: paired renders; a key the gateway has not accepted yet,
// or has finished with, or none at all, sends the person to `/pair`; an
// unreachable gateway says so and offers a retry (it must not look like
// "unpaired", which would send a person with a good key to make a new one), and
// so does a wrong clock (the gateway refuses a signature made at the wrong time
// as it refuses a wrong key): the key is left alone, and `/pair` is offered as a
// separate step the person takes.

import { usePathname, useRouter } from "next/navigation";
import { type ReactNode, useEffect } from "react";
import { GateClock } from "./components/gate-clock";
import { GateSplash } from "./components/gate-splash";
import { GateUnreachable } from "./components/gate-unreachable";
import { KeyStatusProvider, useKeyStatus } from "./key-status";

/** The route that makes a key. */
export const PAIR_PATH = "/pair";

function Gate({ children }: { children: ReactNode }) {
  const pathname = usePathname();
  const router = useRouter();
  const { status, refresh } = useKeyStatus();
  const onPairPage = pathname === PAIR_PATH;
  const needsPairing =
    status.kind === "none" || status.kind === "pending" || status.kind === "ended";

  useEffect(() => {
    if (!onPairPage && needsPairing) router.replace(PAIR_PATH);
  }, [onPairPage, needsPairing, router]);

  if (onPairPage || status.kind === "paired") return <>{children}</>;
  if (status.kind === "unreachable") return <GateUnreachable onRetry={() => void refresh()} />;
  if (status.kind === "clock") {
    return (
      <GateClock
        minutes={Math.round(status.skewSeconds / 60)}
        onRetry={() => void refresh()}
        onPairAgain={() => router.push(PAIR_PATH)}
      />
    );
  }
  return <GateSplash />;
}

export function ConsoleGate({ children }: { children: ReactNode }) {
  return (
    <KeyStatusProvider>
      <Gate>{children}</Gate>
    </KeyStatusProvider>
  );
}
