/**
 * The vocabulary of a status-glyph system — types and shared classes only,
 * no domain data.
 *
 * # What this is
 *
 * A *status system* is one state machine's presentation: a table mapping
 * every state that machine can be in to a glyph, a hue, an attention level
 * and human copy. This module owns the vocabulary and the rendering; the
 * table itself belongs to whoever owns the state machine, which is never
 * this package.
 *
 * ```tsx
 * const ORDER_STATUS = defineStatusSystem({
 *   pending:  { family: "in-flight", silhouette: "circle", mark: "pie-1",
 *               hue: "neutral", filled: false, attention: "quiet",
 *               label: "Pending", tooltip: "Awaiting payment." },
 *   paid:     { family: "terminal", silhouette: "circle", mark: "check",
 *               hue: "success", filled: true, attention: "quiet",
 *               label: "Paid", tooltip: "Settled." },
 *   refunded: { family: "terminal", silhouette: "circle", mark: "bar",
 *               hue: "neutral", filled: true, attention: "quiet",
 *               label: "Refunded", tooltip: "Returned to the payer." },
 * });
 *
 * const OrderStatusPill = createStatusPill(ORDER_STATUS);
 * <OrderStatusPill state="paid" />
 * ```
 *
 * # Every field here is PRESENTATIONAL
 *
 * Never use `family` — or anything else in a `StatusMeta` — to decide
 * whether an action is permitted. Terminality is data, and the server
 * owns it: a UI that greys out "cancel" because its own table says
 * `terminal` will be wrong the first time the state machine changes and
 * nobody remembers this table exists. Propose the action and let the
 * server refuse it.
 *
 * # Why a glyph and not just colour
 *
 * Colour alone fails for the ~8% of men with a colour-vision deficiency,
 * and fails completely in a screenshot pasted into a monochrome ticket.
 * Each state therefore differs in *silhouette* (circle / diamond /
 * square), *interior mark*, and *fill* as well as hue — three redundant
 * channels, so no single one is load-bearing.
 */

/**
 * The three-way split every state machine this system has met so far
 * reduces to: still moving, stopped somewhere that needs a human, or done.
 *
 * `unresolved` is the one worth naming explicitly — it is not a failure
 * and not a success, and systems that only model two outcomes end up
 * calling it whichever is more convenient, which is how "we never learned
 * the outcome" gets reported to a user as "delivered".
 */
export type StatusFamily = "in-flight" | "unresolved" | "terminal";

export type StatusSilhouette = "circle" | "diamond" | "square";

/**
 * The interior mark drawn inside the silhouette. `pie-1`..`pie-3` are the
 * quarter-fill progress wedge; `ring` is a completed stroke with a hollow
 * centre (handed off, awaiting an external answer).
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

/**
 * The saturated hues a status may take.
 *
 * `warning` and `uncertain` are deliberately distinct despite both reading
 * as "attention": `uncertain` means the outcome is unknown, `warning`
 * means a recoverable condition needs a human. `expired` and `parked` are
 * separated from `neutral` for the same reason — a state nobody needs to
 * act on and a state waiting for someone should not look identical.
 */
export type StatusHue =
  | "neutral"
  | "success"
  | "warning"
  | "danger"
  | "uncertain"
  | "expired"
  | "parked";

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
  /** Human-facing name. Sentence case, not the raw enum literal. */
  label: string;
  /** One or two sentences saying what this state means and what happens next. */
  tooltip: string;
}

/** One state machine's complete presentation table. */
export type StatusSystem<S extends string> = Readonly<Record<S, StatusMeta>>;

/**
 * Identity at runtime; exists for inference. Without it a caller must
 * write `const X: StatusSystem<"a" | "b"> = {…}` and repeat every key in
 * the type annotation; with it, `S` is inferred from the object's own keys
 * and `createStatusPill(X)` then accepts exactly those keys and no others.
 */
export function defineStatusSystem<S extends string>(system: StatusSystem<S>): StatusSystem<S> {
  return system;
}

/**
 * Whether a state is terminal *according to its own table* — a
 * presentational question ("should this row stop animating"), never an
 * authorisation one. See this module's own warning above.
 */
export function isTerminalStatus<S extends string>(system: StatusSystem<S>, state: S): boolean {
  return system[state].family === "terminal";
}

/** Hue → the token classes every status surface renders through. */
export const HUE_CLASSES: Record<StatusHue, { fg: string; bg: string; border: string }> = {
  neutral: {
    fg: "text-state-neutral-fg",
    bg: "bg-state-neutral-bg",
    border: "border-state-neutral-border",
  },
  success: {
    fg: "text-state-success-fg",
    bg: "bg-state-success-bg",
    border: "border-state-success-border",
  },
  warning: {
    fg: "text-state-warning-fg",
    bg: "bg-state-warning-bg",
    border: "border-state-warning-border",
  },
  danger: {
    fg: "text-state-danger-fg",
    bg: "bg-state-danger-bg",
    border: "border-state-danger-border",
  },
  uncertain: {
    fg: "text-state-uncertain-fg",
    bg: "bg-state-uncertain-bg",
    border: "border-state-uncertain-border",
  },
  expired: {
    fg: "text-state-expired-fg",
    bg: "bg-state-expired-bg",
    border: "border-state-expired-border",
  },
  parked: {
    fg: "text-state-parked-fg",
    bg: "bg-state-parked-bg",
    border: "border-state-parked-border",
  },
};
