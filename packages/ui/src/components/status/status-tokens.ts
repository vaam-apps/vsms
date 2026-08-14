/**
 * The eleven-state status system (design doc §0.1, §4). Every message state
 * `messages_state_enum_check` can actually produce, transcribed verbatim —
 * `rejected` included, which an earlier brief omitted.
 *
 * `family` and every other field here are PRESENTATIONAL ONLY (design doc
 * §0.2: "terminality is data, not code"). Never use this table to decide
 * whether an action (cancel, re-enqueue, replay) is permitted — that
 * decision belongs to the API and its `SM001` response. The UI proposes,
 * Postgres decides.
 *
 * Job states (`job_state_transitions`: pending/running/succeeded/failed/
 * dead/cancelled) deliberately are NOT mapped here yet. The design doc
 * marks that mapping `OPEN` pending exactly this schema read (done — see
 * `schema/migrations/postgres/0002_bootstrap/up.sql`), but assigns the
 * actual glyph/hue judgement calls to its own follow-up task (§8, B3) —
 * notably, a job's `failed` is *retryable* (`failed -> pending` is a legal
 * edge) and is therefore not equivalent to a message's terminal `failed`,
 * which needs its own reasoning rather than a silent copy-paste here.
 *
 * English only (console-redesign.md D10, English-only constraint 1):
 * `labelFr` is deleted outright, not merely unused, and `labelEn`/
 * `tooltipEn` are renamed to plain `label`/`tooltip` — with the French
 * fields gone, a stray reader is a compile error rather than silently-dead
 * code. This does not foreclose localisation forever: #231 tracks a real
 * `react-i18next` layer for the console later, still English-only by
 * default with additional languages opted into via env-var config — a
 * proper mechanism, not two ad hoc string fields on a domain-semantics
 * table that nothing ever rendered.
 */

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

export type StatusFamily = "in-flight" | "unresolved" | "terminal";

export type StatusSilhouette = "circle" | "diamond" | "square";

/**
 * The interior mark drawn inside the silhouette. `pie-1`..`pie-3` are the
 * quarter-fill progress wedge (design doc §4.3, borrowed from Linear's
 * issue-state glyph); `ring` is a completed stroke with a hollow centre
 * (handed off, awaiting an external answer).
 */
export type StatusMark =
  | "pie-1"
  | "pie-2"
  | "pie-3"
  | "ring"
  | "check"
  | "bar"
  | "clock"
  | "slash"
  | "cross"
  | "question"
  | "pause";

export type StatusHue = "neutral" | "success" | "danger" | "uncertain" | "expired" | "parked";

/** Quiet: glyph only, no fill. Loud: tinted background + border. */
export type StatusAttention = "quiet" | "loud";

export interface StatusMeta {
  family: StatusFamily;
  silhouette: StatusSilhouette;
  mark: StatusMark;
  hue: StatusHue;
  /** Terminal marks sit on a filled silhouette; in-flight/unresolved marks are stroked-only. */
  filled: boolean;
  attention: StatusAttention;
  label: string;
  tooltip: string;
}

/**
 * design doc §4.2, with the owner override applied: `delivered` gets a
 * green pill (§4.5 originally left it untinted; DECISIONS §5 overrides
 * that — "the owner has chosen otherwise"). It stays `quiet` (no
 * background) so a healthy, mostly-`delivered` table doesn't become a wall
 * of green that makes the one red row harder to find — only its glyph and
 * label take the green hue, matching the design doc's own reasoning for
 * why `delivered` is quiet, just with colour restored.
 */
export const MESSAGE_STATUS_META: Record<MessageState, StatusMeta> = {
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
};

export function isTerminalMessageState(state: MessageState): boolean {
  return MESSAGE_STATUS_META[state].family === "terminal";
}

/**
 * #56: the follow-up this file's own module doc named — `job_state_
 * transitions` (`schema/migrations/postgres/0002_bootstrap/up.sql`),
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
export const JOB_STATUS_META: Record<JobState, StatusMeta> = {
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
};

export function isTerminalJobState(state: JobState): boolean {
  return JOB_STATUS_META[state].family === "terminal";
}

/**
 * #55: `attempt_state_transitions` (`schema/migrations/postgres/
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
export const ATTEMPT_STATUS_META: Record<AttemptState, StatusMeta> = {
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
};

export function isTerminalAttemptState(state: AttemptState): boolean {
  return ATTEMPT_STATUS_META[state].family === "terminal";
}
