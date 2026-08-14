// Dumb component (R6): the Dashboard screen's own top-level layout — the
// `flex flex-col gap-6` column and the `grid ... lg:grid-cols-2` pairing
// for the two trend cards, both moved verbatim out of `dashboard-screen.tsx`.
// Composes the smaller dumb components in this directory; owns no data
// fetching or business rules of its own, only where things sit.

import { InlineBanner } from "@vsms/ui";
import { BalanceCard } from "./balance-card";
import { DashboardHeader } from "./dashboard-header";
import { DeliveryRateCard, type OperatorRow } from "./delivery-rate-card";
import { OutboxFootnote } from "./outbox-footnote";
import { ScopeBanner } from "./scope-banner";
import { StatTiles } from "./stat-tiles";
import { type ThroughputBucket, ThroughputCard } from "./throughput-card";
import { type Ucs2Bucket, Ucs2Card } from "./ucs2-card";

export interface DashboardViewProps {
  refetchIntervalMs: number;
  appScoped: boolean;
  errorMessage: string | null;
  isLoading: boolean;
  queueDepth: string;
  jobBacklog: string;
  outboxDepth: string;
  stuckMessages: string;
  stuckMessagesCount: number;
  buckets: (ThroughputBucket & Ucs2Bucket)[];
  throughputDelta: number | null;
  currentRatio: number | null;
  previousRatio: number | null;
  ucs2Jumped: boolean;
  operatorRows: OperatorRow[];
  allOperatorsQuiet: boolean;
}

export function DashboardView({
  refetchIntervalMs,
  appScoped,
  errorMessage,
  isLoading,
  queueDepth,
  jobBacklog,
  outboxDepth,
  stuckMessages,
  stuckMessagesCount,
  buckets,
  throughputDelta,
  currentRatio,
  previousRatio,
  ucs2Jumped,
  operatorRows,
  allOperatorsQuiet,
}: DashboardViewProps) {
  return (
    <div className="flex flex-col gap-6">
      <DashboardHeader refetchIntervalMs={refetchIntervalMs} />

      <ScopeBanner appScoped={appScoped} />

      {errorMessage != null && (
        <InlineBanner variant="danger">{`Couldn't load the dashboard: ${errorMessage}`}</InlineBanner>
      )}

      <StatTiles
        isLoading={isLoading}
        queueDepth={queueDepth}
        jobBacklog={jobBacklog}
        outboxDepth={outboxDepth}
        stuckMessages={stuckMessages}
        stuckMessagesAccent={stuckMessagesCount > 0}
      />

      <div className="grid grid-cols-1 gap-4 lg:grid-cols-2">
        <ThroughputCard buckets={buckets} delta={throughputDelta} />
        <Ucs2Card
          buckets={buckets}
          currentRatio={currentRatio}
          previousRatio={previousRatio}
          jumped={ucs2Jumped}
        />
      </div>

      <DeliveryRateCard
        rows={operatorRows}
        allOperatorsQuiet={allOperatorsQuiet}
        stuckMessages={stuckMessagesCount}
      />

      <BalanceCard />

      <OutboxFootnote />
    </div>
  );
}
