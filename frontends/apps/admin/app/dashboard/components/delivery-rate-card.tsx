// Dumb component (R6): the "Delivery rate by operator" card. `OPERATOR_LABELS`
// is presentational label text for the operator this component renders,
// not business logic — kept beside the markup that uses it, the same way
// `providers-screen.tsx`'s `StatePill` keeps its own state labels.

import { Card, CardBody, CardHeader, InlineBanner } from "@vsms/ui";
import { formatCount, formatPercent } from "../format";

export type Operator = "mtn" | "orange" | "camtel" | "nexttel" | "unknown";

const OPERATOR_LABELS: Record<Operator, string> = {
  mtn: "MTN",
  orange: "Orange",
  camtel: "Camtel",
  nexttel: "Nexttel",
  unknown: "Unknown",
};

export interface OperatorRow {
  operator: Operator;
  delivered: number;
  terminalTotal: number;
}

export interface DeliveryRateCardProps {
  rows: OperatorRow[];
  allOperatorsQuiet: boolean;
  stuckMessages: number;
}

export function DeliveryRateCard({
  rows,
  allOperatorsQuiet,
  stuckMessages,
}: DeliveryRateCardProps) {
  return (
    <Card>
      <CardHeader
        title="Delivery rate by operator"
        meta="trailing 24 hours, terminal messages only"
      />
      <CardBody>
        {allOperatorsQuiet && (
          <p className="text-caption text-muted-foreground">
            No terminal messages in the last 24 hours for this app.
          </p>
        )}
        {rows.length > 0 && (
          <div className="flex flex-col gap-3">
            {rows.map((row) => {
              const ratio = row.delivered / row.terminalTotal;
              return (
                <div key={row.operator} className="flex items-center gap-2 sm:gap-3">
                  <span className="w-14 shrink-0 text-caption text-foreground sm:w-20">
                    {OPERATOR_LABELS[row.operator]}
                  </span>
                  <div className="h-2 min-w-8 flex-1 overflow-hidden rounded-full bg-surface-3">
                    <div
                      className="h-full rounded-full bg-state-success-fg"
                      style={{ width: `${Math.round(ratio * 100)}%` }}
                    />
                  </div>
                  <span className="w-20 shrink-0 text-right font-mono text-caption text-muted-foreground sm:w-32">
                    {formatPercent(ratio)} ({formatCount(row.delivered)}/
                    {formatCount(row.terminalTotal)})
                  </span>
                </div>
              );
            })}
          </div>
        )}
        <InlineBanner variant="plain" className="mt-3">
          Excludes {formatCount(stuckMessages)} stuck message
          {stuckMessages === 1 ? "" : "s"} (uncertain/undelivered) — see the tile above. Counting
          those as failed would overstate failure; counting them as delivered would be a lie.
        </InlineBanner>
      </CardBody>
    </Card>
  );
}
