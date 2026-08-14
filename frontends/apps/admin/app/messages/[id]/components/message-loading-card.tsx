// Dumb — route-local to the message detail screen (R6). The loading
// placeholder shown while `messages.byId` is in flight.

import { Card, CardBody, Skeleton } from "@vsms/ui";

export function MessageLoadingCard() {
  return (
    <Card>
      <CardBody className="pt-4">
        <Skeleton className="h-4 w-full" />
      </CardBody>
    </Card>
  );
}
