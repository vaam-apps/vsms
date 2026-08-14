// Dumb component (R6): a provider's state as a coloured pill. `STATE_CLASSES`
// moved out of `providers-screen.tsx` (it was a hoisted class map living in
// the view file — the textbook R6 violation) into the one component that
// renders it; still a plain lookup rather than a `cva()` table, since `@vsms/
// admin` has no direct `class-variance-authority` dependency today and
// adding one for a single four-variant pill wasn't worth the new dependency
// wiring in this PR.

import type { ProviderState } from "../provider-types";

const STATE_CLASSES: Record<ProviderState, string> = {
  active: "border-state-success-border bg-state-success-bg text-state-success-fg",
  degraded: "border-state-uncertain-border bg-state-uncertain-bg text-state-uncertain-fg",
  disabled: "border-state-danger-border bg-state-danger-bg text-state-danger-fg",
  draining: "border-state-uncertain-border bg-state-uncertain-bg text-state-uncertain-fg",
};

export function StatePill({ state }: { state: ProviderState }) {
  return (
    <span className={`rounded-sm border px-1.5 py-0.5 text-caption ${STATE_CLASSES[state]}`}>
      {state}
    </span>
  );
}
