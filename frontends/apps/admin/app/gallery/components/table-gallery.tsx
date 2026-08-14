"use client";

// Route-local (R6): moved verbatim out of `page.tsx`.

import {
  Button,
  Card,
  InlineEmptyState,
  LiveRow,
  type MessageState,
  Skeleton,
  StatusPill,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@vsms/ui";
import { useState } from "react";
import { Section } from "./section";

const DEMO_ROWS: Array<{ id: string; state: MessageState; recipient: string; version: number }> = [
  { id: "cs_msg_001", state: "delivered", recipient: "+237 6 77 12 34 56", version: 3 },
  { id: "cs_msg_002", state: "uncertain", recipient: "+237 6 91 22 10 09", version: 2 },
  {
    id: "cs_msg_003_a_deliberately_long_client_ref_to_check_overflow_handling",
    state: "queued",
    recipient: "+237 6 55 40 18 77",
    version: 1,
  },
];

export function TableGallery() {
  const [tick, setTick] = useState(0);
  return (
    <Section
      title="Table + LiveRow"
      description="Status column first (§6.4). Click the button to trigger a 240ms wash on the first row, as if its state had just changed — nothing else in the row moves. Third row's id is deliberately long, to check overflow/wrap behaviour rather than only ever testing with tidy fixture data."
    >
      <Button variant="secondary" size="sm" onClick={() => setTick((t) => t + 1)}>
        Simulate a state change on row 1
      </Button>
      <Card>
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Status</TableHead>
              <TableHead>Recipient</TableHead>
              <TableHead>Id</TableHead>
              <TableHead align="end">Version</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {DEMO_ROWS.map((row, i) => (
              <LiveRow key={row.id} washTrigger={i === 0 ? tick : row.version} washHue="success">
                <TableCell>
                  <StatusPill state={row.state} />
                </TableCell>
                <TableCell mono>{row.recipient}</TableCell>
                <TableCell mono>{row.id}</TableCell>
                <TableCell align="end" mono>
                  {row.version}
                </TableCell>
              </LiveRow>
            ))}
          </TableBody>
        </Table>
      </Card>

      <p className="text-caption text-muted-foreground">
        Error state (a failed query, inline — never a placard):
      </p>
      <div className="rounded-sm border border-state-danger-border bg-state-danger-bg px-3 py-2 text-caption text-state-danger-fg">
        Couldn't load webhook attempts: sms-api returned 500.
      </div>

      <InlineEmptyState
        message="No webhook attempts match the current filters."
        action={{ label: "Clear filters", onClick: () => {} }}
      />
      <div className="flex flex-col gap-1">
        <p className="text-caption text-muted-foreground">Loading skeleton (static, no shimmer):</p>
        <Skeleton className="h-10 w-full" />
        <Skeleton className="h-10 w-full" />
      </div>
    </Section>
  );
}
