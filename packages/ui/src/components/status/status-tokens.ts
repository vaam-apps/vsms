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
  labelEn: string;
  labelFr: string;
  tooltipEn: string;
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
    labelEn: "Accepted",
    labelFr: "Acceptée",
    tooltipEn: "Received and validated. Not yet queued.",
  },
  queued: {
    family: "in-flight",
    silhouette: "circle",
    mark: "pie-2",
    hue: "neutral",
    filled: false,
    attention: "quiet",
    labelEn: "Queued",
    labelFr: "En file",
    tooltipEn: "Waiting for a dispatch worker to claim it.",
  },
  routed: {
    family: "in-flight",
    silhouette: "circle",
    mark: "pie-3",
    hue: "neutral",
    filled: false,
    attention: "quiet",
    labelEn: "Routed",
    labelFr: "Routée",
    tooltipEn: "A route and provider were chosen. Not yet submitted.",
  },
  submitted: {
    family: "in-flight",
    silhouette: "circle",
    mark: "ring",
    hue: "neutral",
    filled: false,
    attention: "quiet",
    labelEn: "Submitted",
    labelFr: "Soumise",
    tooltipEn: "Handed to the provider. Awaiting a delivery receipt.",
  },
  delivered: {
    family: "terminal",
    silhouette: "circle",
    mark: "check",
    hue: "success",
    filled: true,
    attention: "quiet",
    labelEn: "Delivered",
    labelFr: "Distribuée",
    tooltipEn: "The provider confirmed delivery to the handset.",
  },
  cancelled: {
    family: "terminal",
    silhouette: "circle",
    mark: "bar",
    hue: "neutral",
    filled: true,
    attention: "quiet",
    labelEn: "Cancelled",
    labelFr: "Annulée",
    tooltipEn: "Cancelled before delivery, on request.",
  },
  expired: {
    family: "terminal",
    silhouette: "circle",
    mark: "clock",
    hue: "expired",
    filled: true,
    attention: "loud",
    labelEn: "Expired",
    labelFr: "Expirée",
    tooltipEn: "Passed its validity window before it could be delivered.",
  },
  rejected: {
    family: "terminal",
    silhouette: "circle",
    mark: "slash",
    hue: "danger",
    filled: true,
    attention: "loud",
    labelEn: "Rejected",
    labelFr: "Rejetée",
    tooltipEn: "Refused at acceptance — opt-out, quota, bad sender ID, or malformed.",
  },
  failed: {
    family: "terminal",
    silhouette: "circle",
    mark: "cross",
    hue: "danger",
    filled: true,
    attention: "loud",
    labelEn: "Failed",
    labelFr: "Échouée",
    tooltipEn: "Permanently failed. The provider error is on the timeline.",
  },
  uncertain: {
    family: "unresolved",
    silhouette: "diamond",
    mark: "question",
    hue: "uncertain",
    filled: false,
    attention: "loud",
    labelEn: "Uncertain",
    labelFr: "Incertaine",
    tooltipEn:
      "Sent, but the outcome was never learned. It will not be retried automatically — this deliberately avoids sending a duplicate. Re-send manually only if a duplicate is acceptable.",
  },
  undelivered: {
    family: "unresolved",
    silhouette: "square",
    mark: "pause",
    hue: "parked",
    filled: false,
    attention: "loud",
    labelEn: "Undelivered",
    labelFr: "Non distribuée",
    tooltipEn:
      "The provider could not deliver it. Retryable in principle — but no retry driver is running today (#122), so it will stay here until someone acts.",
  },
};

export function isTerminalMessageState(state: MessageState): boolean {
  return MESSAGE_STATUS_META[state].family === "terminal";
}
