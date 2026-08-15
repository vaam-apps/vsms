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
// 1. `StateTimeline` (`@vsms/ui`, wrapped by `MessageTimelineCard` below)
//    renders only the transitions `buildTimeline` can point a real
//    timestamp at — `accepted`, `submitted` (if it happened), and the
//    current state. It never shows `queued`/`routed` as dated steps,
//    because nothing timestamps them individually.
// 2. `MessageTimelineCard`'s own disclaimer says that plainly, for every
//    message, not just the two special-cased ones.
// 3. `StateTimeline` itself carries built-in annotations for `uncertain`
//    (an `Indeterminate` submit — no DLR was ever received) and
//    `undelivered` (a retryable-failure DLR, no retry driver running
//    today per #122) — both fire automatically whenever the CURRENT state
//    lands there, regardless of whether a receipt exists. See
//    `timeline.test.ts` for the guard that would catch a regression
//    silently dropping either.
//
// The "Delivery receipts" card below the timeline (`ReceiptsCard`) is the
// raw evidence this timeline is reconstructed from — every
// `DeliveryReceipt` row this system has for this message, verbatim,
// including duplicates and out-of-order arrivals. It is NOT a second copy
// of the timeline; a receipt existing doesn't always mean the message's
// state moved (a duplicate `delivered` DLR after the message is already
// `delivered` changes nothing) — see `backends/crates/sms-api/src/dlr.rs`'s
// own `next_state`.

import { trpc } from "@vsms/hooks";
import { isTerminalMessageState, Separator } from "@vsms/ui";
import { MessageDetailHeader } from "./components/message-detail-header";
import { MessageDetailLayout } from "./components/message-detail-layout";
import { MessageLoadingCard } from "./components/message-loading-card";
import { MessageNotFoundCard } from "./components/message-not-found-card";
import { MessageSummaryCard } from "./components/message-summary-card";
import { MessageTimelineCard } from "./components/message-timeline-card";
import { ReceiptsCard } from "./components/receipts-card";
import { buildTimeline } from "./timeline";

export interface MessageDetailScreenProps {
  messageId: string;
}

export function MessageDetailScreen({ messageId }: MessageDetailScreenProps) {
  const byIdQuery = trpc.messages.byId.useQuery({ id: messageId });
  const receiptsQuery = trpc.messages.receipts.useQuery({ id: messageId });

  return (
    <MessageDetailLayout>
      <MessageDetailHeader messageId={messageId} />

      {byIdQuery.isLoading && <MessageLoadingCard />}
      {byIdQuery.error != null && <MessageNotFoundCard />}

      {byIdQuery.data != null && (
        <>
          <MessageSummaryCard message={byIdQuery.data} />
          <MessageTimelineCard
            transitions={buildTimeline(byIdQuery.data)}
            currentState={byIdQuery.data.state}
            isTerminal={isTerminalMessageState(byIdQuery.data.state)}
          />
          <Separator />
          <ReceiptsCard
            receipts={receiptsQuery.data?.receipts}
            isLoading={receiptsQuery.isLoading}
            errorMessage={receiptsQuery.isError ? receiptsQuery.error.message : undefined}
          />
        </>
      )}
    </MessageDetailLayout>
  );
}
