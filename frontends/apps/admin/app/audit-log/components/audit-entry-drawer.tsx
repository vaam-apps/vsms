// Dumb view: a read-only peek at one audit entry. `entry`/`open` are
// separate, and this is always mounted (never conditionally rendered) for
// the identical reason `apps-screen.tsx`'s `AppDetailDrawer` doc gives —
// `vaul`'s close transition needs the drawer still in the DOM for at least
// one frame after `open` flips `false`. `entry` is nullable so this can
// render (closed) before any row has ever been clicked.

import { DetailList, DetailRow, QuickDetailDrawer, TimestampDisplay } from "@vsms/ui";
import { prettyJson } from "../audit-log-format";
import type { AuditLogEntry } from "../types";
import { JsonBlock } from "./json-block";

export function AuditEntryDrawer({
  entry,
  open,
  onClose,
}: {
  entry: AuditLogEntry | null;
  open: boolean;
  onClose: () => void;
}) {
  return (
    <QuickDetailDrawer
      open={open}
      onOpenChange={(next) => !next && onClose()}
      title={entry !== null ? `${entry.model} · ${entry.operation}` : "Audit entry"}
      description={entry !== null && <TimestampDisplay value={entry.occurredAt} />}
    >
      {entry !== null && (
        <div className="flex flex-col gap-4">
          <DetailList>
            <DetailRow label="Event id">
              <span className="truncate font-mono text-caption text-foreground">
                {entry.eventId}
              </span>
            </DetailRow>
            <DetailRow label="Request id">
              <span className="truncate font-mono text-caption text-foreground">
                {entry.requestId ?? <span className="text-subtle-foreground">none</span>}
              </span>
            </DetailRow>
            <DetailRow label="Tenant">
              <span className="truncate font-mono text-caption text-foreground">
                {entry.tenant ?? <span className="text-subtle-foreground">none</span>}
              </span>
            </DetailRow>
          </DetailList>

          <JsonBlock label="Primary key" value={prettyJson(entry.primaryKey)} />
          <JsonBlock label="Actor" value={prettyJson(entry.actor)} />
          <JsonBlock label="Before" value={prettyJson(entry.before)} />
          <JsonBlock label="After" value={prettyJson(entry.after)} />
        </div>
      )}
    </QuickDetailDrawer>
  );
}
