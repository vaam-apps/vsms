// Pure date-range helpers for the messages list's quick-filter buttons and
// the inclusive/exclusive date-only → ISO-8601 boundary conversion
// `messages.list`'s `to` filter needs. Extracted verbatim from
// `messages-screen.tsx` as part of R6 (AGENTS.md).

export function todayIsoDate(): string {
  return new Date().toISOString().slice(0, 10);
}

export function daysAgoIsoDate(days: number): string {
  const date = new Date();
  date.setUTCDate(date.getUTCDate() - days);
  return date.toISOString().slice(0, 10);
}

/** `to` in `messages.list`'s input is exclusive (`@vsms/gateway/
 * messages.ts`'s own doc) — a date-only picker selecting "2026-08-08"
 * should include the whole day, so this steps one day past it. */
export function nextDayIso(dateOnly: string): string {
  const date = new Date(`${dateOnly}T00:00:00.000Z`);
  date.setUTCDate(date.getUTCDate() + 1);
  return date.toISOString();
}
