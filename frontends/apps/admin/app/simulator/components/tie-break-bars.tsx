// Dumb component (R6): the tie-break weighted-range bar. Markup moved
// verbatim out of `simulator-screen.tsx`. Renders the exact `[low, high)`
// ranges the engine itself reported — see `simulator-screen.tsx`'s own
// module doc for why this file never re-derives them.

export interface TieBreakRange {
  routeId: string;
  weight: number;
  low: number;
  high: number;
}

export interface TieBreakBarsProps {
  priority: number;
  draw: number;
  ranges: TieBreakRange[];
  winnerRouteId: string;
}

export function TieBreakBars({ priority, draw, ranges, winnerRouteId }: TieBreakBarsProps) {
  return (
    <div className="rounded-sm border border-edge bg-surface-2 p-3">
      <p className="text-caption text-muted-foreground">
        Tie-break within priority {priority} — draw{" "}
        <span className="font-mono text-foreground">{draw.toFixed(4)}</span>
      </p>
      <div className="mt-2 flex h-6 w-full overflow-hidden rounded-sm border border-edge">
        {ranges.map((range) => {
          const isWinner = range.routeId === winnerRouteId;
          const widthPct = (range.high - range.low) * 100;
          return (
            <div
              key={range.routeId}
              className={
                isWinner
                  ? "flex items-center justify-center border-edge border-r bg-state-success-bg text-state-success-fg last:border-r-0"
                  : "flex items-center justify-center border-edge border-r bg-surface-3 text-muted-foreground last:border-r-0"
              }
              style={{ width: `${widthPct}%` }}
              title={`route ${range.routeId} — weight ${range.weight} — [${range.low.toFixed(3)}, ${range.high.toFixed(3)})`}
            >
              <span className="truncate px-1 font-mono text-[10px]">w={range.weight}</span>
            </div>
          );
        })}
      </div>
    </div>
  );
}
