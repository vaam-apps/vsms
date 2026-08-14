// Dumb component (R6): the "Throughput" trend card. Markup moved verbatim
// out of `dashboard-screen.tsx`; this component owns building the bar
// labels off the raw hourly buckets it's handed (iteration/presentation,
// not a business decision), while `delta` — a derived comparison across
// two specific hours — stays a value the smart component computes and
// hands down, since the same comparison also feeds the UCS-2 card's own
// "jumped" flag.

import { Card, CardBody, CardHeader } from "@vsms/ui";
import { formatCount } from "../format";
import { HourlyBars } from "./hourly-bars";

export interface ThroughputBucket {
  bucketStart: string;
  totalCount: number;
}

export interface ThroughputCardProps {
  buckets: ThroughputBucket[];
  /** `null` when there's no prior hour to compare against yet. */
  delta: number | null;
}

export function ThroughputCard({ buckets, delta }: ThroughputCardProps) {
  const currentHourTotal = buckets.at(-1)?.totalCount ?? 0;

  return (
    <Card>
      <CardHeader title="Throughput" meta="last 6 hours, oldest first" />
      <CardBody>
        {buckets.length > 0 ? (
          <>
            <HourlyBars
              bars={buckets.map((bucket) => ({
                value: bucket.totalCount,
                label: `${new Date(bucket.bucketStart).toLocaleTimeString([], {
                  hour: "2-digit",
                })}: ${formatCount(bucket.totalCount)} messages`,
              }))}
              colorClassName="bg-state-neutral-fg"
            />
            <p className="mt-3 text-body text-foreground">
              {formatCount(currentHourTotal)}{" "}
              <span className="text-caption text-muted-foreground">this hour</span>
            </p>
            <p className="text-caption text-muted-foreground">
              {delta == null
                ? "no prior hour to compare yet"
                : delta === 0
                  ? "flat vs. the previous hour"
                  : `${delta > 0 ? "+" : ""}${formatCount(delta)} vs. the previous hour`}
            </p>
          </>
        ) : (
          <p className="text-caption text-muted-foreground">Loading…</p>
        )}
      </CardBody>
    </Card>
  );
}
