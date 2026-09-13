// Dumb — route-local to the message detail screen (R6). The loading
// placeholder shown while `messages.byId` is in flight. `SkeletonText`,
// not a single flat bar: the real card it stands in for is a status
// header plus a multi-field grid (`MessageSummaryCard`/`MessageFields`),
// and a paragraph-shaped placeholder with a short last line reads as
// "several pieces of content arriving" the way one full-width bar cannot.

import { Card, CardBody, SkeletonText } from "@vaam-apps/ui";

export function MessageLoadingCard() {
  return (
    <Card>
      <CardBody className="pt-4">
        <SkeletonText lines={4} />
      </CardBody>
    </Card>
  );
}
