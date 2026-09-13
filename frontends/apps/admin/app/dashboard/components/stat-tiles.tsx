// Dumb component (R6): the four-tile stat row (queue depth, job backlog,
// outbox depth, stuck messages) plus its loading skeleton. `StatTile`
// (`@vaam-apps/ui`) replaces this file's own hand-rolled `StatCard` — the
// library's own module doc names a dashboard stat tile as exactly the kind
// of duplication it exists to end: `<dl>/<dt>/<dd>` semantics instead of
// two bare `<p>`s, `font-mono tabular-nums` on the value, `min-w-0`
// grid-overflow handling, `line-clamp-2` on the caption, and a `tone`
// driven by the shared `StatusHue` vocabulary rather than this file's own
// one-off `accent === "uncertain"` ternary. `StatTile` already renders its
// own bordered box (`border-edge`/`bg-base-300`/`rounded-box`) — no `Card`
// wrapper around it, or the loaded tiles would carry chrome twice.

import { Card, CardBody, Skeleton, StatTile } from "@vaam-apps/ui";

export interface StatTilesProps {
  isLoading: boolean;
  queueDepth: string;
  jobBacklog: string;
  outboxDepth: string;
  stuckMessages: string;
  stuckMessagesAccent: boolean;
}

export function StatTiles({
  isLoading,
  queueDepth,
  jobBacklog,
  outboxDepth,
  stuckMessages,
  stuckMessagesAccent,
}: StatTilesProps) {
  if (isLoading) {
    return (
      <div className="grid grid-cols-2 gap-4 sm:grid-cols-4">
        {Array.from({ length: 4 }).map((_, i) => (
          // biome-ignore lint/suspicious/noArrayIndexKey: static skeleton tiles, never reordered
          <Card key={i}>
            <CardBody className="pt-4">
              <Skeleton className="h-8 w-20" />
              <Skeleton className="mt-2 h-3 w-32" />
            </CardBody>
          </Card>
        ))}
      </div>
    );
  }

  return (
    <div className="grid grid-cols-2 gap-4 sm:grid-cols-4">
      <StatTile
        label="Queue depth"
        value={queueDepth}
        caption="accepted + queued + routed, right now"
      />
      <StatTile label="Job backlog" value={jobBacklog} caption="pending + running, system-wide" />
      <StatTile
        label="Outbox depth"
        value={outboxDepth}
        caption="webhook attempts pending or in flight"
      />
      <StatTile
        label="Stuck messages"
        value={stuckMessages}
        caption="uncertain + undelivered — never confirmed either way"
        tone={stuckMessagesAccent ? "uncertain" : undefined}
      />
    </div>
  );
}
