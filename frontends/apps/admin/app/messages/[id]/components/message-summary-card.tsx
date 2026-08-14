// Dumb — route-local to the message detail screen (R6). The record's own
// status/id header plus its field grid.

import { Card, CardBody, CardHeader, IdDisplay, StatusPill } from "@vsms/ui";
import type { MessageDetail } from "../message-record";
import { MessageFields } from "./message-fields";

export interface MessageSummaryCardProps {
  message: MessageDetail;
}

export function MessageSummaryCard({ message }: MessageSummaryCardProps) {
  return (
    <Card>
      <CardHeader
        title={<StatusPill state={message.state} />}
        meta={<IdDisplay value={message.id} variant="full" />}
      />
      <CardBody>
        <MessageFields message={message} />
      </CardBody>
    </Card>
  );
}
