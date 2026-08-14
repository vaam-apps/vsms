"use client";

// Route-local (R6): moved verbatim out of `page.tsx`.

import { Card, CardBody, CardHeader, StateTimeline } from "@vsms/ui";
import { Section } from "./section";

export function StateTimelineGallery() {
  return (
    <Section
      title="State timeline — the epic gate"
      description="Diagnose a message without touching SQL. The annotation nodes carry the §4.7 copy for uncertain/undelivered verbatim. Two examples: still in flight (uncertain, annotated) and terminal (delivered, no trailing 'still moving' cap)."
    >
      <div className="flex flex-col gap-4 lg:flex-row">
        <Card className="flex-1">
          <CardHeader title="cs_msg_002" meta="+237 6 91 22 10 09 · MTN" />
          <CardBody>
            <StateTimeline
              currentState="uncertain"
              isTerminal={false}
              transitions={[
                { toState: "accepted", at: "2026-08-08T14:03:07.412Z", actor: "app:vsms-console" },
                { toState: "queued", at: "2026-08-08T14:03:07.690Z" },
                { toState: "routed", at: "2026-08-08T14:03:08.010Z", providerKey: "orange-cm" },
                {
                  toState: "submitted",
                  at: "2026-08-08T14:03:08.312Z",
                  providerKey: "orange-cm",
                  workerNode: "worker-2",
                  attempt: 1,
                  maxAttempts: 3,
                },
                {
                  toState: "uncertain",
                  at: "2026-08-08T14:03:38.312Z",
                  providerKey: "orange-cm",
                  workerNode: "worker-2",
                  attempt: 1,
                  maxAttempts: 3,
                },
              ]}
            />
          </CardBody>
        </Card>
        <Card className="flex-1">
          <CardHeader title="cs_msg_001" meta="+237 6 77 12 34 56 · MTN" />
          <CardBody>
            <StateTimeline
              currentState="delivered"
              isTerminal
              transitions={[
                { toState: "accepted", at: "2026-08-08T14:02:00.000Z", actor: "app:vsms-console" },
                { toState: "queued", at: "2026-08-08T14:02:00.240Z" },
                { toState: "routed", at: "2026-08-08T14:02:00.510Z", providerKey: "orange-cm" },
                {
                  toState: "submitted",
                  at: "2026-08-08T14:02:00.812Z",
                  providerKey: "orange-cm",
                  workerNode: "worker-1",
                  attempt: 1,
                  maxAttempts: 3,
                },
                {
                  toState: "delivered",
                  at: "2026-08-08T14:02:07.100Z",
                  providerKey: "orange-cm",
                },
              ]}
            />
          </CardBody>
        </Card>
      </div>
      <div className="flex flex-col gap-1">
        <p className="text-caption text-muted-foreground">Loading skeleton (no transitions yet):</p>
        <Card>
          <CardBody className="pt-4">
            <StateTimeline currentState="accepted" isTerminal={false} transitions={[]} />
          </CardBody>
        </Card>
      </div>
    </Section>
  );
}
