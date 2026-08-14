// Dumb component (R6): the quick-detail drawer — a narrow, undimmed peek
// at the fields already on the list row plus `key`/`kind` (console-redesign
// §3/D14). Markup moved verbatim out of `providers-screen.tsx`. Owns no
// route (D14) — that decision, and the state backing it, stays in the
// smart component.

import { Button, IdDisplay, QuickDetailDrawer, TimestampDisplay } from "@vsms/ui";
import type { ProviderState } from "../provider-types";
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
        <dl className="flex flex-col gap-3 text-body">
          <div className="flex items-center justify-between gap-3">
            <dt className="text-muted-foreground">State</dt>
            <dd>
              <StatePill state={detail.state} />
            </dd>
          </div>
          <div className="flex items-center justify-between gap-3">
            <dt className="text-muted-foreground">Key</dt>
            <dd className="font-mono text-caption">{detail.key}</dd>
          </div>
          <div className="flex items-center justify-between gap-3">
            <dt className="text-muted-foreground">Kind</dt>
            <dd className="font-mono text-caption">{detail.kind}</dd>
          </div>
          <div className="flex items-center justify-between gap-3">
            <dt className="text-muted-foreground">Healthy</dt>
            <dd>
              {detail.healthy ? (
                <span className="text-state-success-fg">yes</span>
              ) : (
                <span className="text-muted-foreground">no probe yet</span>
              )}
            </dd>
          </div>
          <div className="flex items-center justify-between gap-3">
            <dt className="text-muted-foreground">Max TPS</dt>
            <dd className="font-mono">{detail.maxTps}</dd>
          </div>
          <div className="flex items-center justify-between gap-3">
            <dt className="text-muted-foreground">Cost/segment (XAF)</dt>
            <dd className="font-mono">{detail.costPerSegmentXaf}</dd>
          </div>
          <div className="flex items-center justify-between gap-3">
            <dt className="text-muted-foreground">Updated</dt>
            <dd>
              <TimestampDisplay value={detail.updatedAt} />
            </dd>
          </div>
        </dl>
      )}
    </QuickDetailDrawer>
  );
}
