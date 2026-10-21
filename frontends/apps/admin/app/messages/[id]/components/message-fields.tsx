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

/** A name when one is known, else the id; absent reads "—". */
function NamedId({
  value,
  names,
}: {
  value: string | undefined | null;
  names: ReadonlyMap<string, string>;
}) {
  if (value == null) return <span>—</span>;
  return <>{names.get(value) ?? <IdDisplay value={value} />}</>;
}

export function MessageFields({
  message,
  providerNames,
  routeNames,
}: {
  message: MessageDetail;
  /** Provider display name by id; a caller without `provider:read` passes an empty map. */
  providerNames: ReadonlyMap<string, string>;
  /** Route name by id; a caller without `route:read` passes an empty map. */
  routeNames: ReadonlyMap<string, string>;
}) {
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
            <NamedId value={message.providerId} names={providerNames} />
          </DetailRow>
          <DetailRow label="Route">
            <NamedId value={message.routeId} names={routeNames} />
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
