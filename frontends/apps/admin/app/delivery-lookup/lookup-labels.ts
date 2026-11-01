// Pure module (R6): the words the Delivery lookup puts on screen that are not
// a server's own. Copy that appears in more than one place, or that a test
// should pin, lives here rather than in a component.
//
// The three choice words avoid the names of the controls this console has
// ruled out for these screens (`pairing-ui-no-dropdowns.test.ts` scans the
// route's files for them, copy included): a choice is "chosen", never
// "selected".

import type { DeliveryOutcome } from "@vsms/gateway";
import type { LookupKind } from "./lookup-query";

export interface KindOption {
  value: LookupKind;
  label: string;
  description: string;
}

/** The key a lookup names, as the radio group offers them. */
export const KIND_OPTIONS: readonly KindOption[] = [
  { value: "id", label: "Message id", description: "Exact, and not counted against your budget." },
  {
    value: "ref",
    label: "Client reference",
    description: "The reference the sending app gave the message.",
  },
  { value: "number", label: "Phone number", description: "The whole recipient number." },
];

/** Said once, above the form: who may use the page and what a lookup costs.
 * The API does not say how much of the budget is left, so no count is shown. */
export const BUDGET_NOTE =
  "Needs the delivery:read permission, held by the Support and Auditor roles. A lookup by phone number or client reference counts against 60 an hour per person; a lookup by message id does not. Every lookup is recorded against your account.";

/** The hint under the date inputs. */
export const WINDOW_HINT = "Dates are UTC. Both days are included.";

/** A receipt's outcome in words. `in_flight` is progress the provider reported,
 * not a verdict, and says so. A record, so a new outcome fails to compile here. */
const OUTCOME_LABELS: Record<DeliveryOutcome, string> = {
  delivered: "Delivered",
  uncertain: "Uncertain",
  failed: "Failed",
  expired: "Expired",
  rejected: "Rejected",
  in_flight: "In flight (progress, not an outcome)",
  unknown: "Unknown",
};

export function outcomeLabel(outcome: DeliveryOutcome): string {
  return OUTCOME_LABELS[outcome];
}

/** The results line read out when it changes. Never the typed value. */
export function resultsSummary(count: number): string {
  if (count === 0) return "No message matched";
  return count === 1 ? "1 message found" : `${count} messages found`;
}

/** Under the table when the server stopped at its limit. */
export function truncatedNote(count: number): string {
  return `Showing the newest ${count}. More messages matched: narrow the window.`;
}

/** What "no message matched" can honestly mean, per key. */
export function emptyCaveat(kind: LookupKind): string {
  switch (kind) {
    case "id":
      return "No message has this id, or it is not one this lookup may show.";
    case "ref":
      return "A client reference is matched exactly, within the app named, and is cleared from a message after 90 days.";
    case "number":
      return "A number is matched whole, against messages that are not purged, from the last 90 days. A number that was recorded before the hash pepper last rotated also reads as no match.";
  }
}

/** Shown where the recipient would be on a purged row. */
export const PURGED_LABEL = "Purged";

/** An app's name by id, for the results' App column; an app the list does not
 * carry (a role that cannot list apps) is simply absent, and the column falls
 * back to the id. */
export function appNameIndex(
  apps: readonly { id: string; name: string }[],
): Record<string, string> {
  return Object.fromEntries(apps.map((app) => [app.id, app.name]));
}
