// Dumb component (R6): the gateway could not be reached, so nothing is known
// about the key. Not "unpaired": a person with a good key must not be sent to
// make a new one because of a dropped connection.

import { Button, InlineBanner } from "@vaam-apps/ui";

export interface GateUnreachableProps {
  onRetry: () => void;
}

export function GateUnreachable({ onRetry }: GateUnreachableProps) {
  return (
    <div className="flex min-h-dvh items-center justify-center p-6">
      <div className="flex w-full max-w-md flex-col gap-4">
        <InlineBanner variant="warning">
          The gateway can&rsquo;t be reached from here, so this console can&rsquo;t tell whether its
          key is still good. Nothing has been changed.
        </InlineBanner>
        <div>
          <Button type="button" onClick={onRetry}>
            Try again
          </Button>
        </div>
      </div>
    </div>
  );
}
