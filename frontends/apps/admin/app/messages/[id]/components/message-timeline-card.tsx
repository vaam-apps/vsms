// Dumb — route-local to the message detail screen (R6). Wraps
// `@vaam-apps/ui`'s `StateTimeline` with the card chrome and the disclaimer
// explaining what it does and doesn't prove — see `timeline.ts`'s own
// module doc for the full reasoning this disclaimer is a summary of.

import { Card, CardBody, CardHeader, StateTimeline, type StateTransition } from "@vaam-apps/ui";
import {
  MESSAGE_STATE_ANNOTATIONS,
  MESSAGE_STATUS_META,
  type MessageState,
} from "@/components/status";

export interface MessageTimelineCardProps {
  transitions: StateTransition<MessageState>[];
  currentState: MessageState;
  isTerminal: boolean;
}

export function MessageTimelineCard({
  transitions,
  currentState,
  isTerminal,
}: MessageTimelineCardProps) {
  return (
    <Card>
      <CardHeader title="Timeline" headingLevel={2} meta="Timestamped steps only" />
      <CardBody>
        <StateTimeline
          transitions={transitions}
          system={MESSAGE_STATUS_META}
          annotations={MESSAGE_STATE_ANNOTATIONS}
          currentState={currentState}
          isTerminal={isTerminal}
        />
        <p className="mt-4 text-body-small text-muted-foreground">
          sms-api records when a message was accepted and submitted, and its state now. The queued
          and routed hops in between carry no timestamp, so they are not drawn as steps.
        </p>
      </CardBody>
    </Card>
  );
}
