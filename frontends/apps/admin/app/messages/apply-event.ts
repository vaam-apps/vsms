// The domain reducer backing the messages list's live-poll loop
// (`messages-screen.tsx`) — merges one incoming `messages.onStateChange`
// frame into the current on-screen view.
//
// Extracted verbatim from `messages-screen.tsx` as part of R6 (AGENTS.md).
// **This is the one extraction in this route that must not be rewritten
// while moving**: it backs the live-poll loop that was found live to
// stall under `refetchInterval` (see `messages-screen.tsx`'s own module
// doc for the full story), and a rewrite during a file move is exactly
// how that regression returns. Every line of `applyEvent` below is
// unchanged from the inline version.
//
// See `apply-event.test.ts` for the real merge rules asserted directly:
// in-place update never moves a row, buffering is scroll-gated.

import type { inferRouterOutputs } from "@trpc/server";
import type { AppRouter } from "@vsms/api";
import type { MessageState } from "@vsms/ui";

type RouterOutputs = inferRouterOutputs<AppRouter>;

export type MessageListItem = RouterOutputs["messages"]["list"]["items"][number];
export type StreamFrame = RouterOutputs["messages"]["onStateChange"]["frames"][number];
export type MessageStreamEvent = Extract<StreamFrame, { type: "message" }>;

export interface ReconcileState {
  rows: MessageListItem[];
  pending: MessageListItem[];
}

/** Merges one live state-change event into the current view — see this
 * file's own module doc for the extraction, and `messages-screen.tsx`'s
 * for the reconciliation rules. `null` `stateFilter` means "no state
 * filter active," matching every event. */
export function applyEvent(
  prev: ReconcileState,
  event: MessageStreamEvent,
  stateFilter: MessageState | null,
  scrolledAway: boolean,
): ReconcileState {
  const matchesFilter = stateFilter === null || event.state === stateFilter;
  const inRows = prev.rows.some((row) => row.id === event.id);
  const inPending = prev.pending.some((row) => row.id === event.id);

  function merge(row: MessageListItem): MessageListItem {
    return {
      ...row,
      state: event.state,
      stateReason: event.stateReason ?? undefined,
      providerMessageRef: event.providerMessageRef ?? undefined,
      version: event.version,
      updatedAt: event.occurredAt,
    };
  }

  if (!matchesFilter) {
    // No longer belongs in this filtered view — drop it if it was here.
    if (!inRows && !inPending) return prev;
    return {
      rows: prev.rows.filter((row) => row.id !== event.id),
      pending: prev.pending.filter((row) => row.id !== event.id),
    };
  }

  if (inRows) {
    return { ...prev, rows: prev.rows.map((row) => (row.id === event.id ? merge(row) : row)) };
  }
  if (inPending) {
    return {
      ...prev,
      pending: prev.pending.map((row) => (row.id === event.id ? merge(row) : row)),
    };
  }

  // A genuinely new row for this view. `MessageListItem` doesn't carry
  // every field a stream event lacks (msisdn, clientRef, senderIdValue,
  // encoding) — those are populated as best-effort empty/placeholder
  // values until the next full `messages.list` refetch (a filter change)
  // fills them in properly. `createdAt` is approximated from the event's
  // own timestamp since the stream doesn't carry it either; harmless here
  // because the default sort is insertion-order (new rows always join at
  // the top), never re-derived from this value.
  const placeholder: MessageListItem = {
    id: event.id,
    appId: event.appId,
    msisdn: "",
    operator: event.operator,
    senderIdValue: "",
    class: "transactional",
    state: event.state,
    stateReason: event.stateReason ?? undefined,
    encoding: "gsm7",
    segments: event.segments,
    providerMessageRef: event.providerMessageRef ?? undefined,
    version: event.version,
    createdAt: event.occurredAt,
    updatedAt: event.occurredAt,
  };

  if (scrolledAway) {
    return { ...prev, pending: [placeholder, ...prev.pending] };
  }
  return { ...prev, rows: [placeholder, ...prev.rows] };
}

/** Reseeds the view fresh from an authoritative `messages.list` fetch —
 * used whenever the query key (i.e. the filters) changes. Live events
 * layer on top from here, via `applyEvent`. */
export function resetReconcileState(items: MessageListItem[]): ReconcileState {
  return { rows: items, pending: [] };
}

/** Moves every buffered row from `pending` to the top of `rows` — design
 * doc §6.5 rule 1: the list never auto-scrolls on its own, only in
 * response to this, driven by a click on the "N new" pill. A no-op
 * (same reference back) when nothing is buffered, so a caller doesn't
 * need to guard the call itself. */
export function insertPendingIntoRows(state: ReconcileState): ReconcileState {
  if (state.pending.length === 0) return state;
  return { rows: [...state.pending, ...state.rows], pending: [] };
}
