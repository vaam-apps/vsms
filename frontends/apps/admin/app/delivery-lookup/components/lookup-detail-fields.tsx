// Dumb component (R6): the quick-detail drawer's field list for one looked-up
// message. Every field the lookup returned, and none it did not: there is no
// body, no number beyond the masked one, no cost.

import { DetailList, DetailRow, IdDisplay, TimestampDisplay } from "@vaam-apps/ui";
import { StatusPill } from "@/components/status";
import { outcomeLabel, PURGED_LABEL } from "../lookup-labels";
import type { LookupStatus } from "./lookup-results";

export interface LookupDetailFieldsProps {
  status: LookupStatus;
  appName: string | undefined;
}

function Instant({ value }: { value: string | undefined }) {
  return value === undefined ? <>—</> : <TimestampDisplay value={value} />;
}

export function LookupDetailFields({ status, appName }: LookupDetailFieldsProps) {
  const receipt = status.lastReceipt;
  return (
    <DetailList variant="divided">
      <DetailRow variant="divided" label="Message id">
        {<IdDisplay value={status.messageId} variant="full" />}
      </DetailRow>
      <DetailRow variant="divided" label="State">
        {<StatusPill state={status.state} />}
      </DetailRow>
      <DetailRow variant="divided" label="Recipient">
        {status.purged || status.recipient === undefined ? (
          PURGED_LABEL
        ) : (
          <span className="font-mono">{status.recipient}</span>
        )}
      </DetailRow>
      <DetailRow variant="divided" label="Purged">
        {status.purged ? "Yes: the recipient and the content are gone" : "No"}
      </DetailRow>
      <DetailRow variant="divided" label="App">
        {appName !== undefined ? appName : <IdDisplay value={status.appId} variant="full" />}
      </DetailRow>
      <DetailRow variant="divided" label="Sender">
        {<span className="font-mono">{status.senderIdValue}</span>}
      </DetailRow>
      <DetailRow variant="divided" label="Operator">
        {<span className="font-mono">{status.operator}</span>}
      </DetailRow>
      <DetailRow variant="divided" label="Class">
        {<span className="font-mono">{status.class}</span>}
      </DetailRow>
      <DetailRow variant="divided" label="Encoding">
        {
          <span className="font-mono">{`${status.encoding}, ${status.segments} segment${status.segments === 1 ? "" : "s"}`}</span>
        }
      </DetailRow>
      <DetailRow variant="divided" label="Attempts">
        {status.attempts}
      </DetailRow>
      <DetailRow variant="divided" label="Accepted">
        {<TimestampDisplay value={status.acceptedAt} />}
      </DetailRow>
      <DetailRow variant="divided" label="Scheduled">
        {<Instant value={status.scheduledAt} />}
      </DetailRow>
      <DetailRow variant="divided" label="Submitted">
        {<Instant value={status.submittedAt} />}
      </DetailRow>
      <DetailRow variant="divided" label="Finalized">
        {<Instant value={status.finalizedAt} />}
      </DetailRow>
      <DetailRow variant="divided" label="Expires">
        {<TimestampDisplay value={status.expiresAt} />}
      </DetailRow>
      <DetailRow variant="divided" label="Last receipt">
        {receipt === undefined ? (
          "None yet"
        ) : (
          <span className="flex flex-col gap-0.5">
            <span>{outcomeLabel(receipt.outcome)}</span>
            <span className="font-mono text-muted-foreground">{receipt.rawStatus}</span>
            {receipt.errorCode !== undefined && (
              <span className="font-mono text-muted-foreground">{`error ${receipt.errorCode}`}</span>
            )}
            <span className="text-muted-foreground">
              {`Network ${receipt.networkCode}, received `}
              <TimestampDisplay value={receipt.receivedAt} />
            </span>
          </span>
        )}
      </DetailRow>
    </DetailList>
  );
}
