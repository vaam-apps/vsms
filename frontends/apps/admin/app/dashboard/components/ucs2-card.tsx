// Dumb component (R6): the "UCS-2 ratio" trend card — the epic's own
// acceptance bar ("a sudden jump means someone shipped a template with a ç
// or a smart apostrophe, and it will show up in the bill before anyone
// notices in the UI"), see `dashboard-screen.tsx`'s own module doc.
// `jumped` is a business decision (the 25-point threshold) computed by the
// smart component; this component only renders what it's told.

import { Card, CardBody, CardHeader } from "@vsms/ui";
import { formatPercent } from "../format";
import { HourlyBars } from "./hourly-bars";

export interface Ucs2Bucket {
  bucketStart: string;
  totalCount: number;
  ucs2Count: number;
}

export interface Ucs2CardProps {
  buckets: Ucs2Bucket[];
  /** `null` when there's no prior hour to compare against yet. */
  currentRatio: number | null;
  previousRatio: number | null;
  jumped: boolean;
}

export function Ucs2Card({ buckets, currentRatio, previousRatio, jumped }: Ucs2CardProps) {
  return (
    <Card>
      <CardHeader
        title="UCS-2 ratio"
        meta="last 6 hours, oldest first"
        action={
          jumped ? (
            <span className="rounded-sm border border-state-uncertain-border bg-state-uncertain-bg px-2 py-0.5 text-caption text-state-uncertain-fg">
              jumped
            </span>
          ) : undefined
        }
      />
      <CardBody>
        {buckets.length > 0 ? (
          <>
            <HourlyBars
              bars={buckets.map((bucket) => ({
                value: bucket.totalCount > 0 ? bucket.ucs2Count / bucket.totalCount : null,
                label:
                  bucket.totalCount > 0
                    ? `${new Date(bucket.bucketStart).toLocaleTimeString([], {
                        hour: "2-digit",
                      })}: ${formatPercent(bucket.ucs2Count / bucket.totalCount)} UCS-2 (${bucket.ucs2Count}/${bucket.totalCount})`
                    : `${new Date(bucket.bucketStart).toLocaleTimeString([], {
                        hour: "2-digit",
                      })}: no messages`,
              }))}
              colorClassName="bg-state-uncertain-fg"
            />
            <p className="mt-3 text-body text-foreground">
              {currentRatio == null ? "—" : formatPercent(currentRatio)}{" "}
              <span className="text-caption text-muted-foreground">this hour</span>
            </p>
            <p className="text-caption text-muted-foreground">
              {previousRatio == null
                ? "no prior hour to compare yet"
                : `previous hour: ${formatPercent(previousRatio)}`}
              {jumped && " — a template change or a smart apostrophe/accent is a common cause"}
            </p>
          </>
        ) : (
          <p className="text-caption text-muted-foreground">Loading…</p>
        )}
      </CardBody>
    </Card>
  );
}
