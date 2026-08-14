// Dumb component (R6): the quick-detail drawer's field list, moved verbatim
// out of `opt-outs-screen.tsx` (`OptOutDetailField` included).

import { IdDisplay, MsisdnDisplay, TimestampDisplay } from "@vsms/ui";
import type { ReactNode } from "react";
import type { OptOutListItem } from "./opt-outs-table";

function OptOutDetailField({ label, value }: { label: string; value: ReactNode }) {
  return (
    <div className="flex flex-col gap-0.5 border-edge-subtle border-b py-2 last:border-b-0">
      <dt className="text-caption text-subtle-foreground">{label}</dt>
      <dd className="text-body text-foreground">{value}</dd>
    </div>
  );
}

export interface OptOutDetailFieldsProps {
  row: OptOutListItem;
}

export function OptOutDetailFields({ row }: OptOutDetailFieldsProps) {
  return (
    <dl className="flex flex-col">
      <OptOutDetailField label="MSISDN" value={<MsisdnDisplay value={row.msisdn} />} />
      <OptOutDetailField
        label="MSISDN hash"
        value={<IdDisplay value={row.msisdnHash} variant="full" />}
      />
      <OptOutDetailField label="Source" value={<span className="font-mono">{row.source}</span>} />
      <OptOutDetailField label="Scope" value={<span className="font-mono">{row.scope}</span>} />
      <OptOutDetailField
        label="Reason"
        value={
          row.reason != null ? (
            <span className="whitespace-pre-wrap break-words">{row.reason}</span>
          ) : (
            "—"
          )
        }
      />
      <OptOutDetailField label="Opted out at" value={<TimestampDisplay value={row.optedOutAt} />} />
      <OptOutDetailField label="Recorded at" value={<TimestampDisplay value={row.createdAt} />} />
      <OptOutDetailField label="Id" value={<IdDisplay value={row.id} variant="full" />} />
    </dl>
  );
}
