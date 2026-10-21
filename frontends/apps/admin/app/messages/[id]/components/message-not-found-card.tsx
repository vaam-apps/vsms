// Dumb component (R6): the detail page when the message cannot be read.
// sms-api answers the same for "no such message" and "not yours to read",
// so this says both. The way back is a real link, not a script navigation.

import { Card, CardBody, InlineEmptyState } from "@vaam-apps/ui";
import { ArrowLeft } from "lucide-react";
import { LinkButton } from "@/components/link-button";

export function MessageNotFoundCard() {
  return (
    <Card>
      <CardBody className="flex flex-col items-start gap-4 pt-5">
        <InlineEmptyState message="This message does not exist, or you cannot read it." />
        <LinkButton href="/messages" size="sm">
          <ArrowLeft size={16} aria-hidden="true" />
          Back to messages
        </LinkButton>
      </CardBody>
    </Card>
  );
}
