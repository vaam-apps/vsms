"use client";

import { useEffect, useRef, useState } from "react";
import { cn } from "../../lib/cn";
import { TableRow, type TableRowProps } from "../primitives/table";
import type { StatusHue } from "../status/status-tokens";

export interface LiveRowProps extends TableRowProps {
  /** Any value that changes to trigger a wash — pass the row's `@version`, an exact change key immune to clock skew. */
  washTrigger: string | number;
  /** The destination state's hue, so the wash tints toward where the row landed. */
  washHue?: StatusHue;
}

const WASH_BG_CLASS: Record<StatusHue, string> = {
  neutral: "bg-state-neutral-fg/10",
  success: "bg-state-success-fg/10",
  danger: "bg-state-danger-fg/10",
  uncertain: "bg-state-uncertain-fg/10",
  expired: "bg-state-expired-fg/10",
  parked: "bg-state-parked-fg/10",
};

/**
 * The row-level half of the design doc's `LiveTable`/`LiveRow` contract
 * (§5.3, §6.5 rule 4): "in-place status change never moves a row" — this
 * wraps `TableRow` with the 240ms wash-then-decay on a status change,
 * nothing else moves or resizes. The full `LiveTable` (scroll-position-
 * dependent buffered insertion, the sticky "N new" pill, sort-mode
 * switching) is a later, screen-level task — this is the reusable unit it
 * will be built on, shipped now so the wash behaviour isn't retrofitted
 * per-screen later.
 *
 * Reduced motion (§3.8 rule 3): the wash becomes a static hold instead of
 * a timed decay — the signal must survive, only the animation may not.
 */
export function LiveRow({
  washTrigger,
  washHue = "neutral",
  className,
  children,
  ...props
}: LiveRowProps) {
  const [washing, setWashing] = useState(false);
  const previousTrigger = useRef(washTrigger);
  const reducedMotion =
    typeof window !== "undefined" && window.matchMedia("(prefers-reduced-motion: reduce)").matches;

  useEffect(() => {
    if (previousTrigger.current === washTrigger) return;
    previousTrigger.current = washTrigger;
    setWashing(true);
    const holdMs = reducedMotion ? 1200 : 400 + 240 + 600;
    const timeout = setTimeout(() => setWashing(false), holdMs);
    return () => clearTimeout(timeout);
  }, [washTrigger, reducedMotion]);

  return (
    <TableRow
      className={cn(
        washing && [
          WASH_BG_CLASS[washHue],
          reducedMotion
            ? "transition-none"
            : "transition-colors duration-[var(--dur-state)] ease-out",
        ],
        className,
      )}
      {...props}
    >
      {children}
    </TableRow>
  );
}
