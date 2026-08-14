import { InlineBanner } from "@vsms/ui";
// Dumb — route-local to messages (R6). Shown while the live-poll loop is
// reporting `degraded` frames; no props, fixed copy.

export function DegradedBanner() {
  return <InlineBanner variant="uncertain">Live updates paused — reconnecting.</InlineBanner>;
}
