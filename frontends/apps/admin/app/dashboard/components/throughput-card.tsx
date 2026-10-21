// Dumb component (R6): messages accepted per hour over the last six hours.

import { Card, CardBody, CardHeader } from "@vaam-apps/ui";
import { formatCount, hourOf } from "../format";
import { HourlyBars } from "./hourly-bars";

export interface ThroughputBucket {
  bucketStart: string;
  totalCount: number;
}

export interface ThroughputCardProps {
  buckets: ThroughputBucket[];
}

export function ThroughputCard({ buckets }: ThroughputCardProps) {
  const total = buckets.reduce((sum, bucket) => sum + bucket.totalCount, 0);
  return (
    <Card>
      <CardHeader
        title="Throughput"
        headingLevel={2}
        meta={`${formatCount(total)} messages in the last 6 hours`}
      />
      <CardBody>
        <HourlyBars
          bars={buckets.map((bucket) => ({
            value: bucket.totalCount,
            hour: hourOf(bucket.bucketStart),
            label: `${hourOf(bucket.bucketStart)}:00 — ${formatCount(bucket.totalCount)} messages`,
          }))}
        />
      </CardBody>
    </Card>
  );
}
