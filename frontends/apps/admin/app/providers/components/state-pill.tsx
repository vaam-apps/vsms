// Dumb component (R6): a provider's state as a coloured pill. `STATE_HUES`
// moved out of `providers-screen.tsx` (it was a hoisted class map living in
// the view file — the textbook R6 violation) into the one component that
// renders it; still a plain lookup rather than a `cva()` table, since `@vsms/
// admin` has no direct `class-variance-authority` dependency today and
// adding one for a single four-variant pill wasn't worth the new dependency
// wiring in this PR.

import { cn, HUE_CLASSES, isQuietHue, type StatusHue } from "@vaam-apps/ui";
import type { ProviderState } from "../provider-types";

const STATE_HUES: Record<ProviderState, StatusHue> = {
  active: "success",
  degraded: "uncertain",
  disabled: "danger",
  draining: "uncertain",
};

export function StatePill({ state }: { state: ProviderState }) {
  const hue = STATE_HUES[state];
  const classes = HUE_CLASSES[hue];
  // `success` is a quiet hue (`isQuietHue`) — its `--state-success-bg`/
  // `-border` tokens are `transparent` by design, so painting them
  // unconditionally rendered "active" as an invisible box instead of a
  // pill. Quiet hues draw the same achromatic bordered surface every
  // quiet hue uses instead; loud hues keep their own tint. The
  // foreground colour is unaffected either way.
  const quiet = isQuietHue(hue);
  return (
    <span
      className={cn(
        "rounded-sm border px-1.5 py-0.5 text-caption",
        quiet ? "border-edge bg-surface-2" : [classes.border, classes.bg],
        classes.fg,
      )}
    >
      {state}
    </span>
  );
}
