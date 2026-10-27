// Dumb component (R6): this console's key — who it speaks as, its thumbprint,
// when it expires — and the two ways to end it. Revoking tells the gateway to
// stop accepting the key everywhere, which is what to do when the browser is
// lost or shared; unpairing only forgets the key in this browser, and the
// gateway keeps accepting it until it expires or is revoked from somewhere else.
// Each asks first, inline (`InlineConfirm`): no dialog, no menu.

import { Button, Code, DetailList, DetailRow, InlineBanner, InlineConfirm } from "@vaam-apps/ui";
import type { ReactNode } from "react";

export type PendingAction = "revoke" | "unpair" | undefined;

export interface ConsoleKeyPanelProps {
  /** Who the key speaks as, e.g. "Ada Lovelace (admin)", when it could be read. */
  person?: string | undefined;
  thumbprint: string | undefined;
  /** ISO 8601. */
  pairedAt: string;
  /** ISO 8601. */
  expiresAt: string;
  daysLeft: number;
  /** False when the gateway's row for the key could not be read, in which case
   *  the key cannot be revoked from here. */
  canRevoke: boolean;
  pending: PendingAction;
  working: boolean;
  error: ReactNode;
  onAsk: (action: Exclude<PendingAction, undefined>) => void;
  onCancel: () => void;
  onConfirm: () => void;
}

function when(iso: string): string {
  return new Date(iso).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });
}

export function ConsoleKeyPanel({
  person,
  thumbprint,
  pairedAt,
  expiresAt,
  daysLeft,
  canRevoke,
  pending,
  working,
  error,
  onAsk,
  onCancel,
  onConfirm,
}: ConsoleKeyPanelProps) {
  return (
    <div className="flex max-w-2xl flex-col gap-6">
      {daysLeft <= 7 && (
        <InlineBanner variant="warning">
          This console&rsquo;s key expires in {daysLeft} {daysLeft === 1 ? "day" : "days"}. A key is
          never extended: when it ends, this page asks you to pair the console again.
        </InlineBanner>
      )}
      <DetailList>
        {person !== undefined && <DetailRow label="Speaks as">{person}</DetailRow>}
        <DetailRow label="Key thumbprint">
          {thumbprint === undefined ? "Unavailable" : <Code>{thumbprint}</Code>}
        </DetailRow>
        <DetailRow label="Paired">{when(pairedAt)}</DetailRow>
        <DetailRow label="Expires">
          {`${when(expiresAt)} (${daysLeft} ${daysLeft === 1 ? "day" : "days"} left)`}
        </DetailRow>
      </DetailList>

      {pending === undefined ? (
        <div className="flex flex-wrap gap-3">
          <Button
            type="button"
            variant="secondary"
            disabled={!canRevoke}
            title={canRevoke ? undefined : "The gateway's record of this key could not be read"}
            onClick={() => onAsk("revoke")}
          >
            Revoke this key
          </Button>
          <Button type="button" variant="ghost" onClick={() => onAsk("unpair")}>
            Unpair this browser
          </Button>
        </div>
      ) : pending === "revoke" ? (
        <InlineConfirm
          title="Revoke this console's key?"
          description="The gateway stops accepting it everywhere, at once. This browser forgets it and asks you to pair again."
          confirmLabel="Revoke"
          pendingLabel="Revoking"
          pending={working}
          error={error}
          onConfirm={onConfirm}
          onCancel={onCancel}
        />
      ) : (
        <InlineConfirm
          title="Unpair this browser?"
          description="This browser forgets its key. The gateway keeps accepting the key until it expires, so revoke it instead if the browser is lost or shared."
          confirmLabel="Unpair"
          pendingLabel="Unpairing"
          pending={working}
          destructive={false}
          error={error}
          onConfirm={onConfirm}
          onCancel={onCancel}
        />
      )}
    </div>
  );
}
