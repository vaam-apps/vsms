// Dumb component (R6): the "Result" card — winner or "no eligible route",
// plus the tie-break bar when one exists. Markup moved verbatim out of
// `simulator-screen.tsx`.

import { Card, CardBody, CardHeader, IdDisplay, MsisdnDisplay } from "@vsms/ui";
import { TieBreakBars, type TieBreakRange } from "./tie-break-bars";

export interface ResultWinner {
  routeId: string;
  providerId: string;
  failoverRouteId?: string | undefined;
}

export interface ResultTieBreak {
  priority: number;
  draw: number;
  ranges: TieBreakRange[];
  winnerRouteId: string;
}

export interface ResultSummaryCardProps {
  msisdn: string;
  operator: string;
  winner: ResultWinner | undefined;
  tieBreak: ResultTieBreak | undefined;
}

export function ResultSummaryCard({ msisdn, operator, winner, tieBreak }: ResultSummaryCardProps) {
  return (
    <Card>
      <CardHeader title="Result" meta={`Classified operator: ${operator}`} />
      <CardBody className="flex flex-col gap-3">
        <MsisdnDisplay value={msisdn} operator={operator} />

        {winner !== undefined ? (
          <div className="rounded-sm border border-state-success-border bg-state-success-bg px-3 py-3 text-state-success-fg">
            <p className="font-medium text-body">
              Winner: route <IdDisplay value={winner.routeId} />
            </p>
            <p className="mt-1 text-caption">
              Provider <IdDisplay value={winner.providerId} />
              {winner.failoverRouteId !== undefined && (
                <>
                  {" "}
                  · failover route <IdDisplay value={winner.failoverRouteId} />
                </>
              )}
            </p>
          </div>
        ) : (
          <div className="rounded-sm border border-state-uncertain-border bg-state-uncertain-bg px-3 py-3 text-caption text-state-uncertain-fg">
            No eligible route for this candidate — every evaluated route below was excluded,
            disabled, failed a predicate, or had no available provider.
          </div>
        )}

        {tieBreak !== undefined && (
          <TieBreakBars
            priority={tieBreak.priority}
            draw={tieBreak.draw}
            ranges={tieBreak.ranges}
            winnerRouteId={tieBreak.winnerRouteId}
          />
        )}
      </CardBody>
    </Card>
  );
}
