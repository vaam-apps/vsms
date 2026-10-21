// Dumb component (R6): the share of messages that needed UCS-2 per hour.
// A UCS-2 body fits 70 characters to a segment instead of 160, so a jump
// means the same traffic suddenly costs more segments.

import { Card, CardBody, CardHeader, StateChip } from "@vaam-apps/ui";
import { formatPercent, hourOf } from "../format";
import { HourlyBars } from "./hourly-bars";

export interface Ucs2Bucket {
  bucketStart: string;
  totalCount: number;
  ucs2Count: number;
}

export interface Ucs2CardProps {
  buckets: Ucs2Bucket[];
  currentRatio: number | null;
  jumped: boolean;
}

export function Ucs2Card({ buckets, currentRatio, jumped }: Ucs2CardProps) {
  return (
    <Card>
      <CardHeader
        title="UCS-2 share"
        headingLevel={2}
        meta={
          currentRatio == null
            ? "No messages this hour"
            : `${formatPercent(currentRatio)} this hour · 70 characters per segment`
        }
        action={jumped ? <StateChip tone="uncertain">Jumped</StateChip> : undefined}
      />
      <CardBody>
        <HourlyBars
          ceiling={1}
          bars={buckets.map((bucket) => {
            const ratio = bucket.totalCount > 0 ? bucket.ucs2Count / bucket.totalCount : null;
            return {
              value: ratio,
              hour: hourOf(bucket.bucketStart),
              label:
                ratio == null
                  ? `${hourOf(bucket.bucketStart)}:00 — no messages`
                  : `${hourOf(bucket.bucketStart)}:00 — ${formatPercent(ratio)} UCS-2 (${bucket.ucs2Count} of ${bucket.totalCount})`,
            };
          })}
        />
        {jumped && (
          <p className="mt-3 text-body-medium text-muted-foreground">
            Up 25 points or more on the previous hour. A template change, a typographic apostrophe
            or an accented capital is the usual cause.
          </p>
        )}
      </CardBody>
    </Card>
  );
}
