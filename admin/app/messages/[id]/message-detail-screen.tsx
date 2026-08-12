"use client";

// The message detail screen (#50) — a single message via `messages.byId`
// (`@vsms/gateway`'s `getMessageById`, unused by any screen before this
// one), plus its state timeline. See `timeline.ts`'s own module doc for
// the design decision the issue asked for: reconstruct from
// `DeliveryReceipt` rows, not the audit log or a new transition-row model,
// and why that choice does NOT mean pretending to know more than the data
// actually proves.
//
// # What "explicit about what it does and does not know" looks like here
//
// Three layers, each honest about its own limits:
//
// 1. `StateTimeline` (`@vsms/ui`) renders only the transitions
//    `buildTimeline` can point a real timestamp at — `accepted`,
//    `submitted` (if it happened), and the current state. It never shows
//    `queued`/`routed` as dated steps, because nothing timestamps them
//    individually.
// 2. The disclaimer directly under the timeline's header says that
//    plainly, for every message, not just the two special-cased ones.
// 3. `StateTimeline` itself carries built-in annotations for `uncertain`
//    (an `Indeterminate` submit — no DLR was ever received) and
//    `undelivered` (a retryable-failure DLR, no retry driver running
//    today per #122) — both fire automatically whenever the CURRENT state
//    lands there, regardless of whether a receipt exists. See
//    `timeline.test.ts` for the guard that would catch a regression
//    silently dropping either.
//
// The "Delivery receipts" card below the timeline is the raw evidence
// this timeline is reconstructed from — every `DeliveryReceipt` row this
// system has for this message, verbatim, including duplicates and
// out-of-order arrivals. It is NOT a second copy of the timeline; a
// receipt existing doesn't always mean the message's state moved (a
// duplicate `delivered` DLR after the message is already `delivered`
// changes nothing) — see `crates/sms-api/src/dlr.rs`'s own `next_state`.

import type { inferRouterOutputs } from "@trpc/server";
import type { AppRouter } from "@vsms/api";
import { trpc } from "@vsms/hooks";
import {
  Card,
  CardBody,
  CardHeader,
  IdDisplay,
  InlineEmptyState,
  isTerminalMessageState,
  MsisdnDisplay,
  Separator,
  Skeleton,
  StateTimeline,
  StatusPill,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
  ThemeToggle,
  TimestampDisplay,
} from "@vsms/ui";
import type { ReactNode } from "react";
import { buildTimeline } from "./timeline";

type RouterOutputs = inferRouterOutputs<AppRouter>;
type MessageDetail = RouterOutputs["messages"]["byId"];
type DeliveryReceiptSummary = RouterOutputs["messages"]["receipts"]["receipts"][number];

export interface MessageDetailScreenProps {
  messageId: string;
}

function Field({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex flex-col gap-1">
      <p className="text-caption text-subtle-foreground">{label}</p>
      <div className="font-mono text-body text-foreground">{children}</div>
    </div>
  );
}

function MessageFields({ message }: { message: MessageDetail }) {
  return (
    <div className="grid grid-cols-2 gap-4 sm:grid-cols-3">
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

function ReceiptsTable({
  receipts,
  isLoading,
}: {
  receipts: DeliveryReceiptSummary[] | undefined;
  isLoading: boolean;
}) {
  if (isLoading) {
    return (
      <div className="flex flex-col gap-2">
        <Skeleton className="h-4 w-full" />
        <Skeleton className="h-4 w-full" />
      </div>
    );
  }

  if (receipts === undefined || receipts.length === 0) {
    return (
      <InlineEmptyState message="No delivery receipts recorded for this message — see the note above the timeline for what that does and doesn't mean." />
    );
  }

  return (
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead>Outcome</TableHead>
          <TableHead>Raw status</TableHead>
          <TableHead>Error code</TableHead>
          <TableHead>Network</TableHead>
          <TableHead align="end">Received</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {receipts.map((receipt) => (
          <TableRow key={receipt.id}>
            <TableCell mono>{receipt.outcome}</TableCell>
            <TableCell mono>{receipt.rawStatus}</TableCell>
            <TableCell mono>{receipt.errorCode ?? "—"}</TableCell>
            <TableCell mono>{receipt.networkCode}</TableCell>
            <TableCell align="end">
              <TimestampDisplay value={receipt.receivedAt} />
            </TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  );
}

export function MessageDetailScreen({ messageId }: MessageDetailScreenProps) {
  const byIdQuery = trpc.messages.byId.useQuery({ id: messageId });
  const receiptsQuery = trpc.messages.receipts.useQuery({ id: messageId });

  return (
    <main className="mx-auto flex max-w-[1000px] flex-col gap-6 px-6 py-10">
      <header className="flex items-start justify-between gap-4 border-edge border-b pb-6">
        <div>
          <p className="font-mono text-micro text-subtle-foreground tracking-[0.03em]">
            vsms admin console
          </p>
          <h1 className="mt-1 font-medium text-foreground text-title">Message detail</h1>
          <p className="mt-1 max-w-xl text-body text-muted-foreground">{messageId}</p>
        </div>
        <div className="flex shrink-0 items-center gap-3">
          <a
            href="/messages"
            className="text-caption text-muted-foreground underline decoration-edge-strong underline-offset-2 hover:decoration-foreground"
          >
            ← Back to messages
          </a>
          <ThemeToggle />
        </div>
      </header>

      {byIdQuery.isLoading && (
        <Card>
          <CardBody className="pt-4">
            <Skeleton className="h-4 w-full" />
          </CardBody>
        </Card>
      )}

      {byIdQuery.error != null && (
        <Card>
          <CardBody className="pt-4">
            <InlineEmptyState
              message="This message doesn't exist, or belongs to a different app — sms-api can't tell the two apart from this console's own credential (see the messages list's own banner)."
              action={{
                label: "Back to messages",
                onClick: () => window.location.assign("/messages"),
              }}
            />
          </CardBody>
        </Card>
      )}

      {byIdQuery.data != null && (
        <>
          <Card>
            <CardHeader
              title={<StatusPill state={byIdQuery.data.state} />}
              meta={<IdDisplay value={byIdQuery.data.id} variant="full" />}
            />
            <CardBody>
              <MessageFields message={byIdQuery.data} />
            </CardBody>
          </Card>

          <Card>
            <CardHeader
              title="Timeline"
              meta="What sms-api actually timestamps — not a full transition log"
            />
            <CardBody>
              <p className="mb-4 rounded-sm border border-edge bg-surface-2 px-3 py-2 text-caption text-muted-foreground">
                This timeline shows only what sms-api directly timestamps: acceptance, submission
                (if it happened), and the current state below. Intermediate hops —{" "}
                <span className="font-mono">queued</span>, <span className="font-mono">routed</span>{" "}
                — aren't individually recorded anywhere, so they aren't shown as dated steps. A
                state reached with no delivery receipt behind it (see the card below) is shown
                exactly as such, not smoothed into a clean chronology.
              </p>
              <StateTimeline
                transitions={buildTimeline(byIdQuery.data)}
                currentState={byIdQuery.data.state}
                isTerminal={isTerminalMessageState(byIdQuery.data.state)}
              />
            </CardBody>
          </Card>

          <Separator />

          <Card>
            <CardHeader
              title="Delivery receipts"
              meta="Raw DeliveryReceipt evidence, oldest first — including duplicates and out-of-order arrivals"
            />
            <CardBody>
              <ReceiptsTable
                receipts={receiptsQuery.data?.receipts}
                isLoading={receiptsQuery.isLoading}
              />
            </CardBody>
          </Card>
        </>
      )}
    </main>
  );
}
