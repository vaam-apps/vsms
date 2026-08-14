// Dumb component (R6): the per-route evaluations table. Markup moved
// verbatim out of `simulator-screen.tsx`. `PREDICATE_LABELS` is
// presentational label text for the predicate this component renders, kept
// beside the markup that uses it — the same convention
// `dashboard/components/delivery-rate-card.tsx`'s `OPERATOR_LABELS` follows.

import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@vsms/ui";
import { type OutcomeKind, OutcomePill } from "./outcome-pill";

export type PredicateKind = "operator" | "class" | "app_id" | "prefix";

const PREDICATE_LABELS: Record<PredicateKind, string> = {
  operator: "Operator",
  class: "Message class",
  app_id: "App",
  prefix: "Prefix",
};

export interface EvaluationRow {
  routeId: string;
  routeName: string;
  priority: number;
  weight: number;
  outcome: OutcomeKind;
  winningBand: boolean;
  predicateKind?: PredicateKind | undefined;
  predicateExpected?: string | undefined;
  predicateActual?: string | undefined;
  unavailableReason?: string | undefined;
}

export interface EvaluationsTableProps {
  rows: EvaluationRow[];
  winnerRouteId: string | undefined;
}

export function EvaluationsTable({ rows, winnerRouteId }: EvaluationsTableProps) {
  return (
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead align="end" hideBelow="sm">
            Priority
          </TableHead>
          <TableHead align="end" hideBelow="sm">
            Weight
          </TableHead>
          <TableHead>Route</TableHead>
          <TableHead>Outcome</TableHead>
          <TableHead hideBelow="md">Detail</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {rows.map((evaluation) => (
          <TableRow key={evaluation.routeId} selected={evaluation.routeId === winnerRouteId}>
            <TableCell align="end" hideBelow="sm" mono>
              {evaluation.priority}
            </TableCell>
            <TableCell align="end" hideBelow="sm" mono>
              {evaluation.weight}
            </TableCell>
            <TableCell>
              {evaluation.routeName}
              {evaluation.routeId === winnerRouteId && (
                <span className="ml-2 rounded-sm border border-state-success-border bg-state-success-bg px-1.5 py-0.5 text-caption text-state-success-fg">
                  winner
                </span>
              )}
            </TableCell>
            <TableCell>
              <OutcomePill outcome={evaluation.outcome} />
            </TableCell>
            <TableCell hideBelow="md" className="text-caption text-muted-foreground">
              {evaluation.outcome === "predicate_failed" &&
                evaluation.predicateKind !== undefined && (
                  <>
                    {PREDICATE_LABELS[evaluation.predicateKind]}: expected{" "}
                    <span className="font-mono text-foreground">
                      {evaluation.predicateExpected}
                    </span>
                    , candidate is{" "}
                    <span className="font-mono text-foreground">{evaluation.predicateActual}</span>
                  </>
                )}
              {evaluation.outcome === "provider_unavailable" && evaluation.unavailableReason}
              {evaluation.outcome === "eligible" &&
                (evaluation.winningBand
                  ? "in the winning priority band"
                  : "outranked by a higher-priority band")}
              {(evaluation.outcome === "excluded" || evaluation.outcome === "disabled") && "—"}
            </TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  );
}
