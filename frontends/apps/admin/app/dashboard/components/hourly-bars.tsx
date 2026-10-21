// Dumb component (R6): six hourly bars, oldest first, with the hour under
// each and the newest hour drawn in the primary colour so the reader's eye
// lands on "now". `primary` is chrome in this design system, not a status
// hue, so a bar chart in it does not read as a verdict. A bar with no data
// (`value: null`) is a short hollow stub: "no data" is not "zero".

import { cn } from "@vaam-apps/ui";

export interface HourlyBar {
  value: number | null;
  /** The hour, shown under the bar, e.g. "14". */
  hour: string;
  /** The bar's accessible name and native tooltip. */
  label: string;
}

export interface HourlyBarsProps {
  bars: HourlyBar[];
  /** A fixed top of the scale (`1` for ratios), or the tallest bar. */
  ceiling?: number | undefined;
}

export function HourlyBars({ bars, ceiling }: HourlyBarsProps) {
  const max = ceiling ?? Math.max(0.0001, ...bars.map((bar) => bar.value ?? 0));
  return (
    <div className="flex flex-col gap-2">
      <div className="flex h-36 items-end gap-2">
        {bars.map((bar, i) => {
          const last = i === bars.length - 1;
          const pct = bar.value == null ? 0 : Math.max(3, Math.round((bar.value / max) * 100));
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
                  "w-full rounded-t-[10px] rounded-b-[4px] transition-[height] duration-[var(--dur-spatial-default)]",
                  bar.value == null ? "h-1.5 bg-surface-3" : last ? "bg-primary" : "bg-primary/35",
                )}
                style={bar.value == null ? undefined : { height: `${pct}%` }}
              />
            </div>
          );
        })}
      </div>
      <div className="flex gap-2" aria-hidden="true">
        {bars.map((bar, i) => (
          <span
            // biome-ignore lint/suspicious/noArrayIndexKey: fixed-length, oldest-first, never reordered
            key={i}
            className={cn(
              "flex-1 text-center font-mono text-label-small",
              i === bars.length - 1 ? "text-foreground" : "text-muted-foreground",
            )}
          >
            {bar.hour}
          </span>
        ))}
      </div>
    </div>
  );
}
