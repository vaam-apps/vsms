// The console's operational tuning values (R6, AGENTS.md): poll cadences and
// page sizes, validated at boot like every other variable, and — unlike the
// rest — coerced even when `SKIP_ENV_VALIDATION` is set. See `env`'s own
// doc in `index.ts` for the measured failure that makes the second half
// necessary.

import { z } from "zod";

export const TUNING = {
  // The Messages screen's live-update long-poll window.
  MESSAGE_STREAM_POLL_MS: z.coerce.number().int().min(500).default(2000),
  // R6: the Jobs (#56), Workers (#57) and Webhooks (#55) screens' shared
  // diagnostics-poll cadence — one place instead of a `REFETCH_INTERVAL_MS
  // = 5000` const per screen.
  DIAGNOSTICS_POLL_MS: z.coerce.number().int().min(500).default(5000),
  // R6 (`audit-log-screen.tsx`): a page size is a tuning value, not a
  // protocol constant.
  AUDIT_LOG_PAGE_SIZE: z.coerce.number().int().min(1).max(500).default(50),
  // Dashboard (#49): an overview cadence, materially slower than the
  // diagnostics tables'.
  DASHBOARD_REFETCH_INTERVAL_MS: z.coerce.number().int().min(1000).default(15_000),
  // How long a query result counts as fresh in the browser before a remount
  // or a window focus refetches it. Navigating back to a screen inside this
  // window shows its cached data with no request at all; polled screens keep
  // their own interval regardless.
  QUERY_STALE_TIME_MS: z.coerce.number().int().min(0).default(10_000),
} as const;

export type TuningKey = keyof typeof TUNING;

/** One tuning value from a raw environment string, falling back to the
 * schema's own default when the string is missing or unusable. */
export function coerceTuning(key: TuningKey, raw: string | undefined): number {
  const schema = TUNING[key];
  const parsed = schema.safeParse(raw === "" ? undefined : raw);
  return parsed.success ? parsed.data : schema.parse(undefined);
}

/** `env` with every tuning key answered by `coerceTuning` instead of the raw
 * string a validation-skipped `createEnv` hands back. */
export function withTuningCoerced<T extends object>(created: T): T {
  return new Proxy(created, {
    get(target, key, receiver) {
      if (typeof key === "string" && key in TUNING) {
        return coerceTuning(key as TuningKey, process.env[key]);
      }
      return Reflect.get(target, key, receiver);
    },
  });
}
