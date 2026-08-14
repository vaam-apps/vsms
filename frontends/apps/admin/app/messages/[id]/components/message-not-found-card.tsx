// Dumb — route-local to the message detail screen (R6). Shown when
// `messages.byId` errors — a nonexistent id, or one belonging to a
// different app this console's own credential can't see (see the
// messages list's own scope banner).

import { Card, CardBody, InlineEmptyState } from "@vsms/ui";

export function MessageNotFoundCard() {
  return (
    <Card>
      <CardBody className="pt-4">
        <InlineEmptyState
          message="This message doesn't exist, or belongs to a different app — sms-api can't tell the two apart from this console's own credential (see the messages list's own banner)."
          action={{
            label: "Back to messages",
            onClick: () => window.location.assign("/messages"),
          }}
        />
      </CardBody>
    </Card>
  );
}
