// Dumb component (R6): the quick-detail drawer's field list, moved verbatim
// out of `opt-outs-screen.tsx` (`OptOutDetailField` included).

import { DetailList, DetailRow, IdDisplay, MsisdnDisplay, TimestampDisplay } from "@vsms/ui";
import type { OptOutListItem } from "./opt-outs-table";

export interface OptOutDetailFieldsProps {
  row: OptOutListItem;
}

export function OptOutDetailFields({ row }: OptOutDetailFieldsProps) {
  return (
    <DetailList variant="divided">
      <DetailRow variant="divided" label="MSISDN">
        {<MsisdnDisplay value={row.msisdn} />}
      </DetailRow>
      <DetailRow variant="divided" label="MSISDN hash">
        {<IdDisplay value={row.msisdnHash} variant="full" />}
      </DetailRow>
      <DetailRow variant="divided" label="Source">
        {<span className="font-mono">{row.source}</span>}
      </DetailRow>
      <DetailRow variant="divided" label="Scope">
        {<span className="font-mono">{row.scope}</span>}
      </DetailRow>
      <DetailRow variant="divided" label="Reason">
        {row.reason != null ? (
          <span className="whitespace-pre-wrap break-words">{row.reason}</span>
        ) : (
          "—"
        )}
      </DetailRow>
      <DetailRow variant="divided" label="Opted out at">
        {<TimestampDisplay value={row.optedOutAt} />}
      </DetailRow>
      <DetailRow variant="divided" label="Recorded at">
        {<TimestampDisplay value={row.createdAt} />}
      </DetailRow>
      <DetailRow variant="divided" label="Id">
        {<IdDisplay value={row.id} variant="full" />}
      </DetailRow>
    </DetailList>
  );
}
