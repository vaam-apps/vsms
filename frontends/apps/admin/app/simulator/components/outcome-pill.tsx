// Dumb component (R6): a route evaluation's outcome as a coloured pill.
// `OUTCOME_LABELS`/`OUTCOME_CLASSES` moved out of `simulator-screen.tsx` (a
// hoisted class map in the view file — the same shape `providers-screen.tsx`'s
// `STATE_CLASSES` was) into the one component that renders them.

export type OutcomeKind =
  | "excluded"
  | "disabled"
  | "predicate_failed"
  | "provider_unavailable"
  | "eligible";

const OUTCOME_LABELS: Record<OutcomeKind, string> = {
  excluded: "Excluded",
  disabled: "Disabled",
  predicate_failed: "Predicate failed",
  provider_unavailable: "Provider unavailable",
  eligible: "Eligible",
};

const OUTCOME_CLASSES: Record<OutcomeKind, string> = {
  excluded: "border-edge-strong bg-surface-2 text-muted-foreground",
  disabled: "border-edge-strong bg-surface-2 text-muted-foreground",
  predicate_failed: "border-state-uncertain-border bg-state-uncertain-bg text-state-uncertain-fg",
  provider_unavailable: "border-state-danger-border bg-state-danger-bg text-state-danger-fg",
  eligible: "border-state-success-border bg-state-success-bg text-state-success-fg",
};

export function OutcomePill({ outcome }: { outcome: OutcomeKind }) {
  return (
    <span className={`rounded-sm border px-1.5 py-0.5 text-caption ${OUTCOME_CLASSES[outcome]}`}>
      {OUTCOME_LABELS[outcome]}
    </span>
  );
}
