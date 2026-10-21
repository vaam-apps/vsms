// Dumb component (R6): the message's routing and accounting facts as a
// definition list. Ids and references are mono; absent values read "—".

import {
  Card,
  CardBody,
  CardHeader,
  DetailList,
  DetailRow,
  IdDisplay,
  TimestampDisplay,
} from "@vaam-apps/ui";
import type { ReactNode } from "react";
import type { MessageDetail } from "../message-record";

function Mono({ children }: { children: ReactNode }) {
  return <span className="break-all font-mono">{children}</span>;
}

function OptionalId({ value }: { value: string | undefined | null }) {
  return value != null ? <IdDisplay value={value} /> : <span>—</span>;
}

export function MessageFields({ message }: { message: MessageDetail }) {
  return (
    <Card>
      <CardHeader title="Details" headingLevel={2} />
      <CardBody>
        <DetailList variant="divided">
          <DetailRow label="Class">{message.class}</DetailRow>
          <DetailRow label="Client reference">
            <Mono>{message.clientRef ?? "—"}</Mono>
          </DetailRow>
          <DetailRow label="Attempts">
            <Mono>
              {message.attempts} of {message.maxAttempts}
            </Mono>
          </DetailRow>
          <DetailRow label="Provider">
            <OptionalId value={message.providerId} />
          </DetailRow>
          <DetailRow label="Route">
            <OptionalId value={message.routeId} />
          </DetailRow>
          <DetailRow label="Provider reference">
            <Mono>{message.providerMessageRef ?? "—"}</Mono>
          </DetailRow>
          <DetailRow label="Cost">
            <Mono>{message.costXaf} XAF</Mono>
          </DetailRow>
          <DetailRow label="Expires">
            <TimestampDisplay value={message.expiresAt} />
          </DetailRow>
          <DetailRow label="Version">
            <Mono>{message.version}</Mono>
          </DetailRow>
        </DetailList>
      </CardBody>
    </Card>
  );
}
