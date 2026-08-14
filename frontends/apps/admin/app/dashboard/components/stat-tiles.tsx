// Dumb component (R6): the four-tile stat row (queue depth, job backlog,
// outbox depth, stuck messages) plus its loading skeleton. `StatCard` was
// previously a local function inside `dashboard-screen.tsx`; moved here
// verbatim alongside the grid that lays it out, since the two never render
// independently.

import { Card, CardBody, CardHeader, Skeleton } from "@vsms/ui";

interface StatCardProps {
  title: string;
  value: string;
  caption: string;
  accent?: "uncertain" | undefined;
}

function StatCard({ title, value, caption, accent }: StatCardProps) {
  return (
    <Card>
      <CardHeader title={title} />
      <CardBody>
        <p
          className={
            accent === "uncertain"
              ? "font-medium text-title tracking-tight text-state-uncertain-fg"
              : "font-medium text-title tracking-tight text-foreground"
          }
        >
          {value}
        </p>
        <p className="mt-1 text-caption text-muted-foreground">{caption}</p>
      </CardBody>
    </Card>
  );
}

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
      <StatCard
        title="Queue depth"
        value={queueDepth}
        caption="accepted + queued + routed, right now"
      />
      <StatCard title="Job backlog" value={jobBacklog} caption="pending + running, system-wide" />
      <StatCard
        title="Outbox depth"
        value={outboxDepth}
        caption="webhook attempts pending or in flight"
      />
      <StatCard
        title="Stuck messages"
        value={stuckMessages}
        caption="uncertain + undelivered — never confirmed either way"
        accent={stuckMessagesAccent ? "uncertain" : undefined}
      />
    </div>
  );
}
