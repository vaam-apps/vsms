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
      <CardHeader
        title="Timeline"
        meta="What sms-api actually timestamps — not a full transition log"
      />
      <CardBody>
        <p className="mb-4 rounded-sm border border-edge bg-surface-2 px-3 py-2 text-caption text-muted-foreground">
          This timeline shows only what sms-api directly timestamps: acceptance, submission (if it
          happened), and the current state below. Intermediate hops —{" "}
          <span className="font-mono">queued</span>, <span className="font-mono">routed</span> —
          aren't individually recorded anywhere, so they aren't shown as dated steps. A state
          reached with no delivery receipt behind it (see the card below) is shown exactly as such,
          not smoothed into a clean chronology.
        </p>
        <StateTimeline
          transitions={transitions}
          system={MESSAGE_STATUS_META}
          annotations={MESSAGE_STATE_ANNOTATIONS}
          currentState={currentState}
          isTerminal={isTerminal}
        />
      </CardBody>
    </Card>
  );
}
