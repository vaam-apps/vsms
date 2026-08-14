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
