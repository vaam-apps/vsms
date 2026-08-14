// Dumb — route-local to messages (R6). Shown while the live-poll loop is
// reporting `degraded` frames; no props, fixed copy.

export function DegradedBanner() {
  return (
    <div className="rounded-sm border border-state-uncertain-border bg-state-uncertain-bg px-3 py-2 text-caption text-state-uncertain-fg">
      Live updates paused — reconnecting.
    </div>
  );
}
