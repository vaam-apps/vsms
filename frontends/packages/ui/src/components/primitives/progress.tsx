import { cn } from "../../lib/cn";
import type { StatusHue } from "../status/status-tokens";
import { HUE_CLASSES } from "../status/status-tokens";

export interface ProgressProps {
  /** Completed units. Clamped into `[0, max]`. */
  value: number;
  max?: number;
  /** `undefined` renders an indeterminate bar. */
  tone?: StatusHue;
  /** Accessible name, e.g. `"Retry budget used"`. */
  label: string;
  /** Show `3 / 5` beside the bar. */
  showValue?: boolean;
  className?: string | undefined;
}

/**
 * A determinate progress bar for a *bounded, countable* quantity —
 * attempts against a retry budget, rows processed, quota consumed.
 *
 * Not for an unbounded wait: a bar that creeps toward a value it cannot
 * reach is a lie, and `Spinner` is the honest alternative. The `max` is
 * required in spirit for that reason, and defaults to 100 only so a
 * percentage — the one case where the bound is implied — needs no
 * ceremony.
 */
export function Progress({
  value,
  max = 100,
  tone = "neutral",
  label,
  showValue = false,
  className,
}: ProgressProps) {
  const safeMax = max <= 0 ? 1 : max;
  const clamped = Math.min(Math.max(value, 0), safeMax);
  const percent = (clamped / safeMax) * 100;
  const hue = HUE_CLASSES[tone];

  return (
    <div className={cn("flex items-center gap-2", className)}>
      <div
        role="progressbar"
        aria-label={label}
        aria-valuenow={clamped}
        aria-valuemin={0}
        aria-valuemax={safeMax}
        className="h-1.5 min-w-0 flex-1 overflow-hidden rounded-full bg-surface-3"
      >
        <div
          // `currentColor` on a text-token class, so the fill tracks the
          // same hue vocabulary every status surface uses rather than
          // introducing a parallel set of bar colours.
          className={cn("h-full rounded-full bg-current transition-[width] duration-300", hue.fg)}
          style={{ width: `${percent}%` }}
        />
      </div>
      {showValue && (
        <span className="shrink-0 font-mono text-caption text-subtle-foreground tabular-nums">
          {clamped} / {safeMax}
        </span>
      )}
    </div>
  );
}
