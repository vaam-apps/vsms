// Dumb component (R6): the text that went to the handset, drawn as the
// message bubble it became, with its encoding and segment count beside it.
// A purged body (90-day retention, #67) says so rather than looking empty.

import { Card, CardBody, CardHeader } from "@vaam-apps/ui";
import type { MessageDetail } from "../message-record";

export function MessageBodyCard({ message }: { message: MessageDetail }) {
  const segments = `${message.segments} segment${message.segments === 1 ? "" : "s"}`;
  return (
    <Card>
      <CardHeader
        title="Message"
        headingLevel={2}
        meta={`${message.encoding.toUpperCase()} · ${segments} · ${message.bodyLength} characters`}
      />
      <CardBody>
        {message.body != null ? (
          <p className="max-w-prose whitespace-pre-wrap break-words rounded-[20px] rounded-bl-[6px] bg-primary-container px-4 py-3 text-body-large text-on-primary-container">
            {message.body}
          </p>
        ) : (
          <p className="text-body-medium text-muted-foreground">
            The body was purged after the retention window. Its hash is kept.
          </p>
        )}
        {message.stateReason != null && (
          <div className="mt-4 rounded-[12px] bg-surface-2 px-4 py-3">
            <p className="text-label-medium text-muted-foreground">Reason given</p>
            <p className="mt-0.5 break-words font-mono text-body-medium">{message.stateReason}</p>
          </div>
        )}
      </CardBody>
    </Card>
  );
}
