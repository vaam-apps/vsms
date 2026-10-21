// Dumb component (R6): what is waiting right now, each line a link to the
// screen that shows those rows. `Job` has no app boundary, so its count is
// system-wide whoever is reading; the event-outbox's own age and poison-row
// alerts live in Prometheus (`deploy/prometheus/alerts.yml`), not here.

import { Card, CardBody, CardHeader } from "@vaam-apps/ui";
import { ChevronRight } from "lucide-react";
import Link from "next/link";

export interface PipelineRow {
  label: string;
  value: string;
  caption: string;
  href: string;
}

export function PipelineCard({ rows }: { rows: PipelineRow[] }) {
  return (
    <Card>
      <CardHeader title="Waiting now" headingLevel={2} meta="Rows still in flight" />
      <CardBody className="px-2 pb-2">
        <ul className="flex flex-col">
          {rows.map((row) => (
            <li key={row.label}>
              <Link
                href={row.href}
                className="flex items-center gap-4 rounded-[16px] px-3 py-3 transition-colors hover:bg-foreground/5 focus-visible:outline-2 focus-visible:outline-primary"
              >
                <span className="flex min-w-0 flex-1 flex-col">
                  <span className="font-medium text-label-large">{row.label}</span>
                  <span className="truncate text-body-small text-muted-foreground">
                    {row.caption}
                  </span>
                </span>
                <span className="font-mono text-title-large tabular-nums">{row.value}</span>
                <ChevronRight
                  size={18}
                  className="shrink-0 text-muted-foreground"
                  aria-hidden="true"
                />
              </Link>
            </li>
          ))}
        </ul>
      </CardBody>
    </Card>
  );
}
