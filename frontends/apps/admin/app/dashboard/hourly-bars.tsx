"use client";

// A dependency-free bar sparkline for the Dashboard screen (#49). No chart
// library exists anywhere in this workspace (`frontends/apps/admin/package.json`,
// `frontends/packages/ui/package.json` — checked before writing this rather than
// added one for a single screen's two small trends), so this is plain
// divs sized by percentage height, themed with the same CSS variables
// every other screen already uses (`frontends/packages/ui/src/styles/theme.css`).
//
// Deliberately not in `@vsms/ui`: this has exactly one consumer today
// (`dashboard-screen.tsx`), and promoting it to the shared package before
// a second screen needs it would be speculative reuse — the opposite of
// this repo's own "composition over re-implementation" rule, which is
// about reusing what already exists, not about pre-building for a future
// that may not arrive the same shape.

import { cn } from "@vsms/ui";

export interface HourlyBar {
  /** 0–1. A bar at `null` renders hollow/muted — "no data", not "zero". */
  value: number | null;
  /** Shown as the bar's `title` (native tooltip) and under narrow bars on
   * hover-less devices via `aria-label`. */
  label: string;
}

interface HourlyBarsProps {
  bars: HourlyBar[];
  colorClassName: string;
  className?: string;
}

/** Six short bars, oldest first, sized against the tallest non-null value
 * in the set (or against `1` when every bar is already a 0–1 ratio and the
 * caller wants a fixed ceiling — pass `fixedCeiling` in that case). */
export function HourlyBars({ bars, colorClassName, className }: HourlyBarsProps) {
  const max = Math.max(0.0001, ...bars.map((bar) => bar.value ?? 0));

  return (
    <div className={cn("flex h-16 items-end gap-1.5", className)}>
      {bars.map((bar, i) => {
        const pct = bar.value == null ? 0 : Math.max(4, Math.round((bar.value / max) * 100));
        return (
          <div
            // biome-ignore lint/suspicious/noArrayIndexKey: fixed-length, oldest-first, never reordered
            key={i}
            role="img"
            className="flex h-full flex-1 items-end"
            title={bar.label}
            aria-label={bar.label}
          >
            <div
              className={cn(
                "w-full rounded-[2px] transition-[height]",
                bar.value == null ? "bg-surface-3" : colorClassName,
              )}
              style={{ height: bar.value == null ? "6%" : `${pct}%` }}
            />
          </div>
        );
      })}
    </div>
  );
}
