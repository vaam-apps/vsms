// Domain data for the Providers screen (#54), extracted out of
// `providers-screen.tsx` — no JSX, no fetching, just the value universe
// `edit-schema.ts` and `./components/state-pill.tsx` both need.

export const PROVIDER_STATES = ["active", "degraded", "disabled", "draining"] as const;
export type ProviderState = (typeof PROVIDER_STATES)[number];
