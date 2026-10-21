// Dumb component (R6): the detail page's title block. The recipient is the
// title (it is how an operator recognises the message they were asked
// about), with its state beside it, and the id, sender and time under it.

import { IdDisplay, TimestampDisplay } from "@vaam-apps/ui";
import { ArrowLeft } from "lucide-react";
import Link from "next/link";
import { MsisdnDisplay } from "@/components/msisdn-display";
import { PageHeader } from "@/components/page-header";
import { StatusPill } from "@/components/status";
import type { MessageDetail } from "../message-record";

function BackLink() {
  return (
    <Link
      href="/messages"
      className="inline-flex items-center gap-1.5 rounded-full hover:text-foreground focus-visible:outline-2 focus-visible:outline-primary"
    >
      <ArrowLeft size={16} aria-hidden="true" />
      Messages
    </Link>
  );
}

export function MessageHeading({ message }: { message: MessageDetail | undefined }) {
  if (message === undefined) {
    return <PageHeader eyebrow={<BackLink />} title="Message" />;
  }
  return (
    <PageHeader
      eyebrow={<BackLink />}
      title={
        <span className="flex flex-wrap items-center gap-3">
          <MsisdnDisplay value={message.msisdn} operator={message.operator} />
          <StatusPill state={message.state} />
        </span>
      }
      description={
        <span className="flex flex-wrap items-center gap-x-2 gap-y-1">
          <span>From {message.senderIdValue}</span>
          <span aria-hidden="true">·</span>
          <span>
            Accepted <TimestampDisplay value={message.createdAt} />
          </span>
          <span aria-hidden="true">·</span>
          <IdDisplay value={message.id} />
        </span>
      }
    />
  );
}
