// Dumb component (R6), app-wide: a row of `FilterChip`s that act as one
// control ("which state"), named for assistive technology. Wraps on a phone
// rather than scrolling sideways, so every chip stays one tap away.

import type { ReactNode } from "react";

export interface ChipGroupProps {
  /** What the chips choose between, e.g. "Job state". */
  label: string;
  children: ReactNode;
}

export function ChipGroup({ label, children }: ChipGroupProps) {
  return (
    <fieldset className="m-0 flex min-w-0 flex-wrap items-center gap-2 border-0 p-0">
      <legend className="sr-only">{label}</legend>
      {children}
    </fieldset>
  );
}
