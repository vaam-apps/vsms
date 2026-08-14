// Dumb component (R6): the quick-detail drawer — a narrow, undimmed peek
// at the fields already on the list row plus `key`/`kind` (console-redesign
// §3/D14). Markup moved verbatim out of `providers-screen.tsx`. Owns no
// route (D14) — that decision, and the state backing it, stays in the
// smart component.

import { Button, IdDisplay, QuickDetailDrawer, TimestampDisplay } from "@vsms/ui";
import type { ProviderState } from "../provider-types";
import { DetailList, DetailRow } from "./detail-row";
import { StatePill } from "./state-pill";

export interface QuickDetail {
  id: string;
  displayName: string;
  state: ProviderState;
  key: string;
  kind: string;
  healthy: boolean;
  maxTps: number;
  costPerSegmentXaf: string;
  updatedAt: string;
}

export interface ProviderQuickDetailProps {
  open: boolean;
  detail: QuickDetail | undefined;
  onClose: () => void;
  onEdit: (id: string) => void;
}

export function ProviderQuickDetail({ open, detail, onClose, onEdit }: ProviderQuickDetailProps) {
  return (
    <QuickDetailDrawer
      open={open}
      onOpenChange={(nextOpen) => !nextOpen && onClose()}
      title={detail?.displayName ?? "Provider"}
      description={detail !== undefined && <IdDisplay value={detail.id} variant="full" />}
      footer={
        <>
          <Button type="button" variant="ghost" size="sm" onClick={onClose}>
            Close
          </Button>
          <Button
            type="button"
            size="sm"
            onClick={() => {
              if (detail === undefined) return;
              onEdit(detail.id);
            }}
          >
            Edit
          </Button>
        </>
      }
    >
      {detail !== undefined && (
        <DetailList>
          <DetailRow label="State">
            <StatePill state={detail.state} />
          </DetailRow>
          <DetailRow label="Key">
            <span className="font-mono text-caption">{detail.key}</span>
          </DetailRow>
          <DetailRow label="Kind">
            <span className="font-mono text-caption">{detail.kind}</span>
          </DetailRow>
          <DetailRow label="Healthy">
            {detail.healthy ? (
              <span className="text-state-success-fg">yes</span>
            ) : (
              <span className="text-muted-foreground">no probe yet</span>
            )}
          </DetailRow>
          <DetailRow label="Max TPS">
            <span className="font-mono">{detail.maxTps}</span>
          </DetailRow>
          <DetailRow label="Cost/segment (XAF)">
            <span className="font-mono">{detail.costPerSegmentXaf}</span>
          </DetailRow>
          <DetailRow label="Updated">
            <TimestampDisplay value={detail.updatedAt} />
          </DetailRow>
        </DetailList>
      )}
    </QuickDetailDrawer>
  );
}
