// Dumb component (R6): delivered over terminal, per operator, trailing 24h.
// Messages whose outcome was never learned (uncertain, undelivered) are left
// out of both sides: counting them as failed would overstate failure, and as
// delivered would be a claim nobody made.

import { Card, CardBody, CardHeader, InlineEmptyState } from "@vaam-apps/ui";
import { formatCount, formatPercent } from "../format";

export type Operator = "mtn" | "orange" | "camtel" | "nexttel" | "unknown";

const OPERATOR_LABELS: Record<Operator, string> = {
  mtn: "MTN",
  orange: "Orange",
  camtel: "Camtel",
  nexttel: "Nexttel",
  unknown: "Other",
};

export interface OperatorRow {
  operator: Operator;
  delivered: number;
  terminalTotal: number;
}

export interface DeliveryRateCardProps {
  rows: OperatorRow[];
  stuckMessages: number;
}

export function DeliveryRateCard({ rows, stuckMessages }: DeliveryRateCardProps) {
  return (
    <Card>
      <CardHeader
        title="Delivery by operator"
        headingLevel={2}
        meta={`Last 24 hours · ${formatCount(stuckMessages)} with no outcome yet, not counted`}
      />
      <CardBody>
        {rows.length === 0 ? (
          <InlineEmptyState message="No message reached a final state in the last 24 hours." />
        ) : (
          <ul className="flex flex-col gap-4">
            {rows.map((row) => {
              const ratio = row.delivered / row.terminalTotal;
              return (
                <li key={row.operator} className="flex flex-col gap-1.5">
                  <div className="flex items-baseline justify-between gap-3">
                    <span className="font-medium text-label-large">
                      {OPERATOR_LABELS[row.operator]}
                    </span>
                    <span className="font-mono text-body-medium tabular-nums">
                      {formatPercent(ratio)}
                      <span className="text-muted-foreground">
                        {" "}
                        · {formatCount(row.delivered)} of {formatCount(row.terminalTotal)}
                      </span>
                    </span>
                  </div>
                  <div className="h-2 overflow-hidden rounded-full bg-surface-3">
                    <div
                      className="h-full rounded-full bg-state-success-fg"
                      style={{ width: `${Math.round(ratio * 100)}%` }}
                    />
                  </div>
                </li>
              );
            })}
          </ul>
        )}
      </CardBody>
    </Card>
  );
}
