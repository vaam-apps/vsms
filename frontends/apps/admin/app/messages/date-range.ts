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

/** The one-tap ranges the list offers. */
export type DatePreset = "today" | "7d" | "30d";

/** The `from`/`to` pair a preset selects. */
export function presetRange(preset: DatePreset): { from: string; to: string } {
  const to = todayIsoDate();
  if (preset === "today") return { from: to, to };
  return { from: daysAgoIsoDate(preset === "7d" ? 7 : 30), to };
}

/** Which preset, if any, the current range is — so its chip shows as
 * selected, and stops showing the moment either date is edited by hand. */
export function activePreset(from: string | null, to: string | null): DatePreset | null {
  for (const preset of ["today", "7d", "30d"] as const) {
    const range = presetRange(preset);
    if (range.from === from && range.to === to) return preset;
  }
  return null;
}
