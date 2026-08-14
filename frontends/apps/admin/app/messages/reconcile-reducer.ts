// Names the three transitions `messages-screen.tsx`'s on-screen row state
// can make, so they're explicit rather than implied by which `useState`
// setter a given call site happens to reach (R6, AGENTS.md: "several
// values that change together → `useReducer`"). `rows` and `pending`
// always change together — every branch below is a thin dispatch onto the
// already-tested pure functions in `apply-event.ts`; this file adds no
// merge logic of its own.

import type { MessageState } from "@vsms/ui";
import type { MessageListItem, MessageStreamEvent, ReconcileState } from "./apply-event";
import { applyEvent, insertPendingIntoRows, resetReconcileState } from "./apply-event";

export type ReconcileAction =
  /** A fresh `messages.list` fetch landed — reseed from it. */
  | { type: "reset"; items: MessageListItem[] }
  /** One live `messages.onStateChange` event arrived. */
  | {
      type: "event";
      event: MessageStreamEvent;
      stateFilter: MessageState | null;
      scrolledAway: boolean;
    }
  /** The "N new" pill was clicked — flush the buffer into view. */
  | { type: "insertPending" };

export function reconcileReducer(state: ReconcileState, action: ReconcileAction): ReconcileState {
  switch (action.type) {
    case "reset":
      return resetReconcileState(action.items);
    case "event":
      return applyEvent(state, action.event, action.stateFilter, action.scrolledAway);
    case "insertPending":
      return insertPendingIntoRows(state);
    default:
      return state;
  }
}

export const INITIAL_RECONCILE_STATE: ReconcileState = { rows: [], pending: [] };
