// #50: turning a `Message` row plus its `DeliveryReceipt` evidence into
// the ordered `StateTransition[]` `@vsms/ui`'s `StateTimeline` component
// renders. This is the load-bearing decision the issue asked for — see
// the module doc below for which of the three named options this is and
// why, and for the two documented gaps a real timeline must not paper
// over.
//
// # The decision: reconstruct from `DeliveryReceipt`, not the audit log
// or a new transition-row model
//
// `Message` carries only its *current* `state` — no history lives on the
// row. #50 named three ways to get one back:
//
//   1. Reconstruct from `DeliveryReceipt` rows (simple, incomplete).
//   2. Read the `@@audit` log (a truer history, but needs a policy that
//      admits the console's principal AND an API route that doesn't
//      exist — `cratestack_audit` has no delegate, no REST surface, and
//      reading it from application code would be a new R1 exception for
//      a table that also carries every OTHER model's before/after
//      snapshots, not just `Message`'s).
//   3. Add explicit transition rows (the most honest, and a real schema
//      change — a new model, a new trigger, a migration).
//
// This module takes option 1, deliberately. Not because it's the
// cheapest — because the *incompleteness* option 2/3 would paper over is
// exactly the thing #50's own acceptance criterion forbids hiding: "the
// timeline must not imply a linear history it cannot prove." A
// `DeliveryReceipt` row is real, timestamped evidence a provider callback
// arrived and what it said — nothing here invents a transition it can't
// point at real evidence for.
//
// # What this function refuses to fabricate
//
// `Message` has no `queuedAt`/`routedAt` column — nothing anywhere
// timestamps those two hops individually. So this function never
// synthesises `queued`/`routed` entries with a guessed timestamp; it only
// ever emits a transition it has a real timestamp for:
//
//   - `accepted @ createdAt` — always known, it's the row's own creation.
//   - `submitted @ submittedAt` — only if the message ever reached
//     `submitted` (dispatch.rs stamps this field on submit; a message
//     rejected at acceptance never has one).
//   - the message's CURRENT state @ `finalizedAt ?? updatedAt` — always
//     appended last (skipped only if it would exactly duplicate the
//     `submitted` entry above, i.e. the message hasn't moved since).
//
// That third entry is what makes the two documented gaps visible rather
// than silently absent:
//
//   - A message that went `submitted -> uncertain` via
//     `ProviderError::Indeterminate` (backends/crates/sms-provider's own taxonomy)
//     has NO `DeliveryReceipt` at all — the outcome was never learned.
//     This function still emits `uncertain @ updatedAt` as the current
//     state, and `StateTimeline`'s own built-in annotation for `uncertain`
//     (`frontends/packages/ui/src/components/bespoke/state-timeline.tsx`) renders
//     directly under it, verbatim: "The outcome was never learned... this
//     message will not be resubmitted." Nothing here writes that copy —
//     it already existed in the component, keyed on `toState`, and pointing
//     the timeline at the true current state is what makes it fire for
//     real data instead of only the gallery's hand-authored example.
//   - A DLR that raced the submit response (`providerMessageRef` not yet
//     persisted, per `backends/crates/sms-api/src/dlr.rs`'s own module doc) is
//     silently dropped — no `DeliveryReceipt` row for it, ever. The
//     message eventually reaches a real terminal state via
//     `expire_stale`, and that's exactly what this function shows: the
//     terminal state, with no receipt "explaining" it, because none
//     exists. It does not invent one.
//
// # Receipts are shown separately, not folded into the timeline
//
// `MessageDetailScreen` renders `receipts` as its own evidence list below
// the timeline, verbatim (outcome/rawStatus/errorCode/networkCode/
// receivedAt/occurredAt) — every row this system has, including
// duplicates and out-of-order arrivals (the chaos suite deliberately
// produces both). This function does NOT try to map each receipt onto a
// `StateTransition` of its own. It can't, honestly:
// `backends/crates/sms-api/src/dlr.rs::next_state` shows a `DeliveryOutcome::Failed`
// receipt drives either `-> undelivered` or `-> failed` depending on the
// message's state at the moment that specific DLR was ingested — a value
// this row alone does not record, and this project's own release notes
// (AGENTS.md) record more than one bug that came from exactly this kind
// of after-the-fact guess. The message's own current-state entry above
// already carries a timestamp close to whichever receipt actually drove
// it (the same transaction updates `Message.updatedAt`); the receipts
// list is where the exact evidence — including receipts that never
// changed the state at all — lives.
//
// See `timeline.test.ts` for the two verified gap cases and every
// boundary this module doc claims.

import type { StateTransition } from "@vsms/ui";

/**
 * Only the fields `buildTimeline` actually reads, off `MessageRecord`.
 *
 * `submittedAt`/`finalizedAt` are typed `| null` here, not just
 * `| undefined` — found live, driving a real message through this exact
 * screen against `just demo`, not assumed from `@vsms/gateway/
 * messages.ts`'s own module doc (which claimed "all of [the nullable
 * fields] are omitted from JSON when null," confirmed there only for
 * `stateReason`, a `String?` column). A real `Indeterminate`-submit
 * message (`routed -> uncertain` directly, per `backends/crates/sms-api/src/
 * dlr.rs`'s own doc — it never touches `submitted` at all) came back over
 * REST as `"submittedAt": null`, a real JSON `null`, not an omitted key.
 * The first version of this function checked `!== undefined` only, which
 * is `true` for `null` — so it rendered a bogus "Submitted" entry dated
 * the Unix epoch (`new Date(null)`). `MessageRecord`'s own declared type
 * (`string | undefined`) didn't catch this at compile time either:
 * `getJson`'s `parsed as T` is a raw cast with no runtime validation, so a
 * `DateTime?` column reaching this layer as `null` was never something
 * `tsc` could see.
 *
 * **Correction, #221:** `@vsms/gateway`'s own type declaration being wrong
 * about this was named at the time as "a separate, pre-existing issue, not
 * fixed in this PR." It's fixed now — `frontends/packages/gateway/src/json.ts` is
 * the single seam that converts sms-api's `null` to `undefined` for every
 * response this package parses, so `MessageRecord.submittedAt`/
 * `finalizedAt` genuinely are `string | undefined` (never `null`) by the
 * time they reach this file. The `| null` half of this type and the loose
 * `!= null` check below are deliberately left in place anyway: this
 * function's own contract shouldn't depend on trusting one particular
 * upstream package to have normalized its input, and `timeline.test.ts`'s
 * `submittedAt: null` case exercises `buildTimeline` directly, bypassing
 * `@vsms/gateway` entirely — a real regression test, not a hypothetical.
 */
export interface TimelineMessageInput {
  state: StateTransition["toState"];
  createdAt: string;
  submittedAt?: string | null | undefined;
  finalizedAt?: string | null | undefined;
  updatedAt: string;
  attempts: number;
  maxAttempts: number;
}

export function buildTimeline(message: TimelineMessageInput): StateTransition[] {
  const transitions: StateTransition[] = [{ toState: "accepted", at: message.createdAt }];

  if (message.submittedAt != null) {
    transitions.push({
      toState: "submitted",
      at: message.submittedAt,
      attempt: message.attempts,
      maxAttempts: message.maxAttempts,
    });
  }

  const lastKnown = transitions[transitions.length - 1];
  if (lastKnown === undefined || lastKnown.toState !== message.state) {
    transitions.push({
      toState: message.state,
      at: message.finalizedAt ?? message.updatedAt,
      attempt: message.attempts,
      maxAttempts: message.maxAttempts,
    });
  }

  return transitions;
}
