/**
 * vsms's own state machines, as status systems.
 *
 * These three tables used to live in `@vaam-apps/ui` itself. They moved
 * here when that package was generalised for public release: a table of
 * SMS delivery-receipt tooltips is this application's domain knowledge,
 * not a component library's, and shipping it to every consumer of the
 * library would have put "Handed to the provider. Awaiting a delivery
 * receipt." in the autocomplete of an unrelated product.
 *
 * What stayed in the package is the *mechanism* — `StatusMeta`, the glyph
 * geometry, the hue classes, and `createStatusPill`, which binds one of
 * these tables to a pill component whose `state` prop accepts exactly
 * that machine's literals. The three near-identical pill components that
 * used to exist (`StatusPill`, `JobStatusPill`, `AttemptStatusPill`, one
 * per machine, differing only in which table they indexed) are now three
 * `createStatusPill` calls at the bottom of this file. Their own doc
 * comments argued — correctly — that the machines differ in meaning even
 * where a literal matches, and that distinction is preserved: each keeps
 * its own table, its own component, and its own state type, so a
 * `JobState` still cannot be passed to a message pill.
 *
 * Every field in these tables is PRESENTATIONAL. Never read `family` to
 * decide whether an action is permitted — the API and its `SM001`
 * response own that. The UI proposes, Postgres decides.
 */

import {
  createStatusPill,
  defineStatusSystem,
  isTerminalStatus,
  type StatusMeta,
} from "@vaam-apps/ui";

export const MESSAGE_STATES = [
  "accepted",
  "queued",
  "routed",
  "submitted",
  "delivered",
  "uncertain",
  "undelivered",
  "failed",
  "expired",
  "rejected",
  "cancelled",
] as const;

export type MessageState = (typeof MESSAGE_STATES)[number];

/**
 * design doc §4.2, with the owner override applied: `delivered` gets a
 * green pill (§4.5 originally left it untinted; DECISIONS §5 overrides
 * that — "the owner has chosen otherwise"). It stays `quiet` (no
 * background) so a healthy, mostly-`delivered` table doesn't become a wall
 * of green that makes the one red row harder to find — only its glyph and
 * label take the green hue, matching the design doc's own reasoning for
 * why `delivered` is quiet, just with colour restored.
 */
export const MESSAGE_STATUS_META: Record<MessageState, StatusMeta> = defineStatusSystem({
  accepted: {
    family: "in-flight",
    silhouette: "circle",
    mark: "pie-1",
    hue: "neutral",
    filled: false,
    attention: "quiet",
    label: "Accepted",
    tooltip: "Received and validated. Not yet queued.",
  },
  queued: {
    family: "in-flight",
    silhouette: "circle",
    mark: "pie-2",
    hue: "neutral",
    filled: false,
    attention: "quiet",
    label: "Queued",
    tooltip: "Waiting for a dispatch worker to claim it.",
  },
  routed: {
    family: "in-flight",
    silhouette: "circle",
    mark: "pie-3",
    hue: "neutral",
    filled: false,
    attention: "quiet",
    label: "Routed",
    tooltip: "A route and provider were chosen. Not yet submitted.",
  },
  submitted: {
    family: "in-flight",
    silhouette: "circle",
    mark: "ring",
    hue: "neutral",
    filled: false,
    attention: "quiet",
    label: "Submitted",
    tooltip: "Handed to the provider. Awaiting a delivery receipt.",
  },
  delivered: {
    family: "terminal",
    silhouette: "circle",
    mark: "check",
    hue: "success",
    filled: true,
    attention: "quiet",
    label: "Delivered",
    tooltip: "The provider confirmed delivery to the handset.",
  },
  cancelled: {
    family: "terminal",
    silhouette: "circle",
    mark: "bar",
    hue: "neutral",
    filled: true,
    attention: "quiet",
    label: "Cancelled",
    tooltip: "Cancelled before delivery, on request.",
  },
  expired: {
    family: "terminal",
    silhouette: "circle",
    mark: "clock",
    hue: "expired",
    filled: true,
    attention: "loud",
    label: "Expired",
    tooltip: "Passed its validity window before it could be delivered.",
  },
  rejected: {
    family: "terminal",
    silhouette: "circle",
    mark: "slash",
    hue: "danger",
    filled: true,
    attention: "loud",
    label: "Rejected",
    tooltip: "Refused at acceptance — opt-out, quota, bad sender ID, or malformed.",
  },
  failed: {
    family: "terminal",
    silhouette: "circle",
    mark: "cross",
    hue: "danger",
    filled: true,
    attention: "loud",
    label: "Failed",
    tooltip: "Permanently failed. The provider error is on the timeline.",
  },
  uncertain: {
    family: "unresolved",
    silhouette: "diamond",
    mark: "question",
    hue: "uncertain",
    filled: false,
    attention: "loud",
    label: "Uncertain",
    tooltip:
      "Sent, but the outcome was never learned. It will not be retried automatically — this deliberately avoids sending a duplicate. Re-send manually only if a duplicate is acceptable.",
  },
  undelivered: {
    family: "unresolved",
    silhouette: "square",
    mark: "pause",
    hue: "parked",
    filled: false,
    attention: "loud",
    label: "Undelivered",
    tooltip:
      "The provider could not deliver it. Retryable in principle — but no retry driver is running today (#122), so it will stay here until someone acts.",
  },
});

export function isTerminalMessageState(state: MessageState): boolean {
  return isTerminalStatus(MESSAGE_STATUS_META, state);
}

/**
 * #56: the follow-up this file's own module doc named — `job_state_
 * transitions` (`backends/migrations/postgres/0002_bootstrap/up.sql`),
 * verbatim: `pending`, `running`, `succeeded`, `failed`, `dead`,
 * `cancelled`. `dead` replaces what would otherwise be a second
 * `failed`-shaped terminal state — see [`JOB_STATUS_META`]'s own comment
 * on `failed` for why the two are deliberately not styled the same way a
 * naive copy from [`MESSAGE_STATUS_META`] would.
 */
export const JOB_STATES = [
  "pending",
  "running",
  "succeeded",
  "failed",
  "dead",
  "cancelled",
] as const;

export type JobState = (typeof JOB_STATES)[number];

/**
 * Job states are not equivalent to message states, even where the names
 * match — reusing [`MESSAGE_STATUS_META`]'s glyph choices verbatim would
 * be the same "silent copy-paste" this file's own module doc already
 * warns against for `dead`/`failed`:
 *
 * - **`failed` is retryable, not terminal.** `failed -> pending` is a
 *   legal edge (`jobs::apply_failure`'s own automatic backoff) — a job
 *   only reaches `dead` once `maxAttempts` is exhausted. Styled
 *   `unresolved`/`uncertain`, the same family `undelivered` (a message
 *   state that *is* retryable, just with no driver yet) already uses,
 *   never `danger`/terminal the way a message's own `failed` is. In
 *   practice this state is close to unobservable — `apply_failure` writes
 *   `running -> failed` and then, within the same function call, `failed
 *   -> {pending, dead}` — but the table has to classify every state
 *   `JobState` admits, not just the ones a poll is likely to catch mid-
 *   flight.
 * - **`dead` is the real terminal failure** — attempts exhausted, and
 *   (#56) the one state `requeueJob` accepts. Styled `danger`/loud, the
 *   analogue of a message's own `failed`.
 */
export const JOB_STATUS_META: Record<JobState, StatusMeta> = defineStatusSystem({
  pending: {
    family: "in-flight",
    silhouette: "circle",
    mark: "pie-1",
    hue: "neutral",
    filled: false,
    attention: "quiet",
    label: "Pending",
    tooltip: "Waiting to be claimed, or waiting out a retry backoff.",
  },
  running: {
    family: "in-flight",
    silhouette: "circle",
    mark: "pie-3",
    hue: "neutral",
    filled: false,
    attention: "quiet",
    label: "Running",
    tooltip: "Claimed by a worker and currently executing.",
  },
  succeeded: {
    family: "terminal",
    silhouette: "circle",
    mark: "check",
    hue: "success",
    filled: true,
    attention: "quiet",
    label: "Succeeded",
    tooltip: "Completed without error.",
  },
  failed: {
    family: "unresolved",
    silhouette: "diamond",
    mark: "clock",
    hue: "uncertain",
    filled: false,
    attention: "loud",
    label: "Failed (retrying)",
    tooltip:
      "The last attempt errored. Not terminal — it will retry automatically after a backoff, unless attempts are exhausted (then it moves to Dead).",
  },
  dead: {
    family: "terminal",
    silhouette: "circle",
    mark: "cross",
    hue: "danger",
    filled: true,
    attention: "loud",
    label: "Dead",
    tooltip: "Every attempt failed and the retry budget is exhausted. Requeue to try again.",
  },
  cancelled: {
    family: "terminal",
    silhouette: "circle",
    mark: "bar",
    hue: "neutral",
    filled: true,
    attention: "quiet",
    label: "Cancelled",
    tooltip: "Cancelled before it ran.",
  },
});

export function isTerminalJobState(state: JobState): boolean {
  return isTerminalStatus(JOB_STATUS_META, state);
}

/**
 * #55: `attempt_state_transitions` (`backends/migrations/postgres/
 * 0002_bootstrap/up.sql`), verbatim — `pending`, `delivering`, `succeeded`,
 * `failed`, `dead`. Same "not equivalent to `MessageState` even where a
 * name matches" caution `JOB_STATUS_META`'s own doc gives: `failed` here is
 * `hooks.rs`'s own retry-with-backoff state, not a terminal one — the same
 * shape `JobState.failed` already has, and styled identically to it for
 * that reason (`unresolved`/`uncertain`, not `danger`).
 */
export const ATTEMPT_STATES = ["pending", "delivering", "succeeded", "failed", "dead"] as const;

export type AttemptState = (typeof ATTEMPT_STATES)[number];

/**
 * - **`pending`** — claimable on the next `hooks` tick (a fresh attempt, or
 *   one whose backoff has elapsed).
 * - **`delivering`** — currently being POSTed. A row can also sit here
 *   because a worker crashed mid-attempt with a stale lease — `claim.rs`'s
 *   own crash-reclaim resumes it without double-counting `attempts`
 *   (AGENTS.md's #40 section) — this table has no separate state for that,
 *   the same way `Message.routed` covers both "in flight" and "reclaimable".
 * - **`succeeded`** — the endpoint returned 2xx. Terminal.
 * - **`failed`** — the last attempt errored and it will retry automatically
 *   after a backoff, unless `attempts` is exhausted (then `dead`). Not
 *   terminal, exactly the retry-with-backoff shape `JOB_STATUS_META.failed`
 *   already documents for `Job`.
 * - **`dead`** — `maxAttempts` exhausted, or an immediate 410 Gone
 *   (`hooks.rs`'s own doc). Terminal, and (#43) the one state
 *   `replayWebhookAttempt` accepts alongside `failed`.
 */
export const ATTEMPT_STATUS_META: Record<AttemptState, StatusMeta> = defineStatusSystem({
  pending: {
    family: "in-flight",
    silhouette: "circle",
    mark: "pie-1",
    hue: "neutral",
    filled: false,
    attention: "quiet",
    label: "Pending",
    tooltip: "Claimable on the next delivery tick, or waiting out a retry backoff.",
  },
  delivering: {
    family: "in-flight",
    silhouette: "circle",
    mark: "pie-3",
    hue: "neutral",
    filled: false,
    attention: "quiet",
    label: "Delivering",
    tooltip: "Currently being POSTed to the endpoint.",
  },
  succeeded: {
    family: "terminal",
    silhouette: "circle",
    mark: "check",
    hue: "success",
    filled: true,
    attention: "quiet",
    label: "Succeeded",
    tooltip: "The endpoint returned 2xx.",
  },
  failed: {
    family: "unresolved",
    silhouette: "diamond",
    mark: "clock",
    hue: "uncertain",
    filled: false,
    attention: "loud",
    label: "Failed (retrying)",
    tooltip:
      "The last attempt errored. Not terminal — it will retry automatically after a backoff, unless attempts are exhausted (then it moves to Dead).",
  },
  dead: {
    family: "terminal",
    silhouette: "circle",
    mark: "cross",
    hue: "danger",
    filled: true,
    attention: "loud",
    label: "Dead",
    tooltip: "Every attempt failed and the retry budget is exhausted. Replay to try again.",
  },
});

export function isTerminalAttemptState(state: AttemptState): boolean {
  return isTerminalStatus(ATTEMPT_STATUS_META, state);
}

/**
 * `Message.class`'s four values (`schema.cstack`'s `MessageClass` enum),
 * verbatim. Hoisted here (R6, AGENTS.md) because it was duplicated
 * byte-for-byte in three screens independently (`app/page.tsx`'s composer,
 * `app/simulator/simulator-screen.tsx`, `app/routes/routes-screen.tsx`) —
 * the domain vocabulary belongs beside the rest of the status/domain
 * tables in this file, not copy-pasted per screen.
 */
export const MESSAGE_CLASSES = ["otp", "transactional", "notification", "marketing"] as const;

export type MessageClass = (typeof MESSAGE_CLASSES)[number];

/**
 * The three bound pills. Each accepts only its own machine's literals, so
 * `<JobStatusPill state="delivered" />` is a compile error — `delivered`
 * is a message state, and a job never reaches it.
 */
export const StatusPill = createStatusPill(MESSAGE_STATUS_META);
export const JobStatusPill = createStatusPill(JOB_STATUS_META);
export const AttemptStatusPill = createStatusPill(ATTEMPT_STATUS_META);

/**
 * The two states that look exactly like bugs to anyone who does not
 * already know the product decision behind them. Without this annotation
 * on the timeline, the operator's next move is to open psql — precisely
 * the outcome the message-detail screen exists to prevent.
 *
 * Passed to `StateTimeline`'s `annotations` prop. It used to be a
 * hard-coded table inside that component; it is domain knowledge, so it
 * lives with the state machine it explains.
 */
export const MESSAGE_STATE_ANNOTATIONS: Partial<Record<MessageState, string>> = {
  uncertain:
    "The outcome was never learned. providerMessageRefAlt was stamped with the message id so a late DLR can still correlate. This message will not be resubmitted — a deliberate trade against sending a duplicate OTP.",
  undelivered:
    'The provider said "not delivered", not "never". undelivered -> queued is a legal edge, but no retry driver runs today (#122) — this message will stay here until someone acts.',
};
