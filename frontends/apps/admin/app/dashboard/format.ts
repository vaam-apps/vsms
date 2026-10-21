// Pure formatting helpers for the Dashboard screen (#49), extracted out of
// `dashboard-screen.tsx` per AGENTS.md's R6 ("if something in it could be
// unit-tested without React, it does not belong there"). Moved verbatim —
// see `format.test.ts` for the coverage that extraction makes possible.

const numberFormat = new Intl.NumberFormat("en-US");

export function formatCount(n: number): string {
  return numberFormat.format(n);
}

export function formatPercent(ratio: number): string {
  return `${(ratio * 100).toFixed(ratio >= 0.1 ? 0 : 1)}%`;
}

/** The two-digit local hour of an ISO instant, e.g. "14". Rendered only on
 * the client (the dashboard's data arrives after hydration), so the
 * browser's own zone is the one the operator reads in. */
export function hourOf(iso: string): string {
  return new Date(iso).toLocaleTimeString([], { hour: "2-digit", hour12: false }).slice(0, 2);
}

/** Delivered over terminal across every operator, or `null` when nothing
 * reached a final state. */
export function overallDeliveryRate(
  rows: readonly { delivered: number; terminalTotal: number }[],
): { ratio: number; delivered: number; terminal: number } | null {
  const delivered = rows.reduce((sum, row) => sum + row.delivered, 0);
  const terminal = rows.reduce((sum, row) => sum + row.terminalTotal, 0);
  return terminal === 0 ? null : { ratio: delivered / terminal, delivered, terminal };
}

/** "+8 on the previous hour", "level with the previous hour", or, with no
 * previous hour, `null`. */
export function describeDelta(delta: number | null): string | null {
  if (delta == null) return null;
  if (delta === 0) return "Level with the previous hour";
  return `${delta > 0 ? "+" : "−"}${formatCount(Math.abs(delta))} on the previous hour`;
}
