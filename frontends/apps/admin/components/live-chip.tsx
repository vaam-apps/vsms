// Dumb component (R6), app-wide: says whether a screen is keeping itself
// current (the console polls; it does not push, so the label names the
// cadence rather than claiming "live"). A
// small pulsing dot (still under reduced motion) and a word; "Reconnecting"
// in the uncertain hue when the poll loop has lost the server. Not a
// `StatusPill`: it describes the screen, not a record.

import { cn } from "@vaam-apps/ui";

export function LiveChip({ live, label }: { live: boolean; label?: string | undefined }) {
  return (
    <span
      role="status"
      className={cn(
        "inline-flex h-8 items-center gap-2 rounded-full px-3 font-medium text-label-large",
        live ? "bg-surface-2 text-foreground" : "bg-state-uncertain-bg text-state-uncertain-fg",
      )}
    >
      <span
        aria-hidden="true"
        className={cn(
          "size-2 rounded-full",
          live ? "bg-state-success-fg motion-safe:animate-pulse" : "bg-state-uncertain-fg",
        )}
      />
      {live ? (label ?? "Up to date") : "Reconnecting"}
    </span>
  );
}
