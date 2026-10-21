// Dumb component (R6), app-wide: an M3 filter chip — 32px, 8px corners, an
// outline at rest and a filled `primary-container` with a check mark when
// selected. For a one-tap preset ("Last 7 days", "Failed") beside a full
// control; `aria-pressed` carries the selected state, so a screen reader
// hears a toggle rather than a plain button.

import { Check } from "lucide-react";
import type { ReactNode } from "react";

export interface FilterChipProps {
  selected: boolean;
  onClick: () => void;
  children: ReactNode;
}

export function FilterChip({ selected, onClick, children }: FilterChipProps) {
  return (
    <button
      type="button"
      aria-pressed={selected}
      onClick={onClick}
      className={
        selected
          ? "inline-flex h-8 items-center gap-1.5 rounded-[8px] bg-primary-container pr-3 pl-2 font-medium text-label-large text-on-primary-container transition-colors focus-visible:outline-2 focus-visible:outline-primary"
          : "inline-flex h-8 items-center rounded-[8px] border border-outline-variant px-3 font-medium text-label-large text-on-surface-variant transition-colors hover:bg-foreground/5 focus-visible:outline-2 focus-visible:outline-primary"
      }
    >
      {selected && <Check size={16} aria-hidden="true" />}
      {children}
    </button>
  );
}
