// Dumb — route-local to the message detail screen (R6). The record's own
// fields, laid out as a label/value grid.

import { MsisdnDisplay, TimestampDisplay } from "@vsms/ui";
import type { ReactNode } from "react";
import type { MessageDetail } from "../message-record";

function Field({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex min-w-0 flex-col gap-1">
      <p className="text-caption text-subtle-foreground">{label}</p>
      <div className="break-all font-mono text-body text-foreground">{children}</div>
    </div>
  );
}

export interface MessageFieldsProps {
  message: MessageDetail;
}

export function MessageFields({ message }: MessageFieldsProps) {
  return (
    <div className="grid grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-3">
      <Field label="Recipient">
        <MsisdnDisplay value={message.msisdn} operator={message.operator} />
      </Field>
      <Field label="Sender">{message.senderIdValue}</Field>
      <Field label="Class">{message.class}</Field>
      <Field label="Client ref">{message.clientRef ?? "—"}</Field>
      <Field label="Encoding">
        {message.encoding.toUpperCase()} · {message.segments} segment
        {message.segments === 1 ? "" : "s"}
      </Field>
      <Field label="Attempts">
        {message.attempts} / {message.maxAttempts}
      </Field>
      <Field label="Provider ref">{message.providerMessageRef ?? "—"}</Field>
      <Field label="Route">{message.routeId ?? "—"}</Field>
      <Field label="Provider">{message.providerId ?? "—"}</Field>
      <Field label="Cost (XAF)">{message.costXaf}</Field>
      <Field label="Expires">
        <TimestampDisplay value={message.expiresAt} />
      </Field>
      <Field label="Version">{message.version}</Field>
      {message.stateReason != null && (
        <div className="col-span-full flex flex-col gap-1">
          <p className="text-caption text-subtle-foreground">State reason</p>
          <p className="font-mono text-body text-foreground">{message.stateReason}</p>
        </div>
      )}
      {message.body != null && (
        <div className="col-span-full flex flex-col gap-1">
          <p className="text-caption text-subtle-foreground">Body</p>
          <p className="whitespace-pre-wrap font-mono text-body text-foreground">{message.body}</p>
        </div>
      )}
    </div>
  );
}
