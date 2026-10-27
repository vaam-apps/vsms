"use client";

// This console's key (docs/design/console-pairing.md): who it speaks as, its
// thumbprint and expiry, and how to end it. Revoking calls
// `revokeConsoleKey` (the user's own key; an owner or admin could revoke any,
// from the Users screen's data, not here), then forgets the key. Unpairing only
// forgets it. Either way the gate sees "no key" and sends the person to pair.

import { InlineBanner, ScreenStack } from "@vaam-apps/ui";
import { trpc } from "@vsms/hooks";
import { useReducer } from "react";
import { PageHeader } from "@/components/page-header";
import { deleteDeviceKey } from "@/lib/device-key";
import { daysLeft } from "@/lib/key-lifecycle";
import { forgetSealer } from "@/lib/sealer";
import { useKeyStatus } from "../key-status";
import { ConsoleKeyPanel, type PendingAction } from "./components/console-key-panel";

type Ask = { pending: PendingAction; working: boolean; error: string | undefined };
type Event =
  | { type: "ask"; action: Exclude<PendingAction, undefined> }
  | { type: "cancel" }
  | { type: "working" }
  | { type: "failed"; message: string };

function reduce(state: Ask, event: Event): Ask {
  switch (event.type) {
    case "ask":
      return { pending: event.action, working: false, error: undefined };
    case "cancel":
      return { pending: undefined, working: false, error: undefined };
    case "working":
      return { ...state, working: true, error: undefined };
    case "failed":
      return { ...state, working: false, error: event.message };
  }
}

export function ConsoleKeyScreen() {
  const { status, thumbprint, refresh } = useKeyStatus();
  const [ask, dispatch] = useReducer(reduce, {
    pending: undefined,
    working: false,
    error: undefined,
  });
  const revoke = trpc.consoleKey.revoke.useMutation();
  const userId = status.kind === "paired" ? status.meta.userId : "";
  const user = trpc.users.get.useQuery({ id: userId }, { enabled: userId !== "", retry: false });

  const forget = async () => {
    await deleteDeviceKey();
    forgetSealer();
    await refresh();
  };

  const onConfirm = async () => {
    if (status.kind !== "paired") return;
    dispatch({ type: "working" });
    try {
      if (ask.pending === "revoke") await revoke.mutateAsync({ keyId: status.meta.keyId });
      await forget();
    } catch (error) {
      dispatch({
        type: "failed",
        message: error instanceof Error ? error.message : "That did not work. Nothing was changed.",
      });
    }
  };

  const person =
    user.data?.data !== undefined
      ? `${user.data.data.displayName} (${user.data.data.roleKey})`
      : undefined;

  return (
    <ScreenStack>
      <PageHeader
        title="This console"
        description="This browser's key signs every request this console makes. There is no password and no session behind it."
      />
      {status.kind === "paired" ? (
        <ConsoleKeyPanel
          person={person}
          thumbprint={thumbprint}
          pairedAt={status.meta.pairedAt}
          expiresAt={status.meta.expiresAt}
          daysLeft={daysLeft(status.meta, Date.now())}
          canRevoke={status.meta.keyId !== ""}
          pending={ask.pending}
          working={ask.working}
          error={ask.error}
          onAsk={(action) => dispatch({ type: "ask", action })}
          onCancel={() => dispatch({ type: "cancel" })}
          onConfirm={() => void onConfirm()}
        />
      ) : (
        <InlineBanner variant="neutral">Checking this console&rsquo;s key.</InlineBanner>
      )}
    </ScreenStack>
  );
}
