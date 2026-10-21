// Dumb component (R6): the dashboard's layout. `DashboardScreen` fetches and
// derives; this arranges.
//
// The four figures an operator checks first sit in the one
// `InstrumentPanel` on the screen (the instrument register is for data you
// scan, and a screen where every panel is one has none). Messages sent this
// hour is the emphasised figure: it answers "is traffic flowing" before any
// other number on the page means anything.

import { InlineBanner, InstrumentPanel, Skeleton, StatTile } from "@vaam-apps/ui";
import { SquarePen } from "lucide-react";
import { LinkButton } from "@/components/link-button";
import { PageHeader } from "@/components/page-header";
import { DeliveryRateCard, type OperatorRow } from "./delivery-rate-card";
import { PipelineCard, type PipelineRow } from "./pipeline-card";
import { type ThroughputBucket, ThroughputCard } from "./throughput-card";
import { type Ucs2Bucket, Ucs2Card } from "./ucs2-card";

export interface DashboardFigure {
  value: string;
  caption: string;
}

export interface DashboardViewProps {
  refreshSeconds: number;
  scopeCaption: string;
  errorMessage: string | null;
  isLoading: boolean;
  sentThisHour: DashboardFigure;
  deliveryRate: DashboardFigure;
  queueDepth: DashboardFigure;
  stuck: DashboardFigure & { attention: boolean };
  pipeline: PipelineRow[];
  buckets: (ThroughputBucket & Ucs2Bucket)[];
  currentRatio: number | null;
  ucs2Jumped: boolean;
  operatorRows: OperatorRow[];
  stuckMessagesCount: number;
}

function FigureSkeleton() {
  return (
    <div className="flex flex-col gap-3 rounded-[20px] bg-surface-container-highest p-5">
      <Skeleton className="h-4 w-24" />
      <Skeleton className="h-9 w-20" />
      <Skeleton className="h-3 w-32" />
    </div>
  );
}

export function DashboardView(props: DashboardViewProps) {
  const { isLoading } = props;
  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        title="Overview"
        description={`Traffic, delivery and what is waiting, updated every ${props.refreshSeconds} s.`}
        actions={
          <LinkButton href="/composer" variant="primary">
            <SquarePen size={18} aria-hidden="true" />
            Compose
          </LinkButton>
        }
      />

      {props.errorMessage != null && (
        <InlineBanner variant="danger">{`The dashboard could not load: ${props.errorMessage}`}</InlineBanner>
      )}

      <InstrumentPanel title="This hour" caption={props.scopeCaption}>
        <div className="grid grid-cols-2 gap-3 xl:grid-cols-4">
          {isLoading ? (
            <>
              <FigureSkeleton />
              <FigureSkeleton />
              <FigureSkeleton />
              <FigureSkeleton />
            </>
          ) : (
            <>
              <StatTile
                label="Messages sent"
                value={props.sentThisHour.value}
                caption={props.sentThisHour.caption}
                emphasized
              />
              <StatTile
                label="Delivered, 24 h"
                value={props.deliveryRate.value}
                caption={props.deliveryRate.caption}
              />
              <StatTile
                label="Queue depth"
                value={props.queueDepth.value}
                caption={props.queueDepth.caption}
              />
              <StatTile
                label="No outcome yet"
                value={props.stuck.value}
                caption={props.stuck.caption}
                tone={props.stuck.attention ? "uncertain" : undefined}
              />
            </>
          )}
        </div>
      </InstrumentPanel>

      <div className="grid grid-cols-1 gap-4 lg:grid-cols-[minmax(0,3fr)_minmax(0,2fr)]">
        <ThroughputCard buckets={props.buckets} />
        <PipelineCard rows={props.pipeline} />
      </div>

      <div className="grid grid-cols-1 gap-4 lg:grid-cols-2">
        <DeliveryRateCard rows={props.operatorRows} stuckMessages={props.stuckMessagesCount} />
        <Ucs2Card
          buckets={props.buckets}
          currentRatio={props.currentRatio}
          jumped={props.ucs2Jumped}
        />
      </div>
    </div>
  );
}
