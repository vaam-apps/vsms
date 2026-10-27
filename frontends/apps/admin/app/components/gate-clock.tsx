// Dumb component (R6): this computer's clock is wrong, so the console cannot
// tell whether its key is still good. Not "unpaired": the gateway refuses a
// signature made at the wrong time exactly as it refuses a wrong key, and a key
// that may still be accepted is never replaced on that evidence. Fixing the
// clock is the way forward; pairing again is there for a person who knows the
// key is finished, and is a separate, explicit step.

import { Button, InlineBanner } from "@vaam-apps/ui";

export interface GateClockProps {
  /** Minutes this computer's clock is off by: positive when it is ahead. */
  minutes: number;
  onRetry: () => void;
  onPairAgain: () => void;
}

export function GateClock({ minutes, onRetry, onPairAgain }: GateClockProps) {
  return (
    <div className="flex min-h-dvh items-center justify-center p-6">
      <div className="flex w-full max-w-md flex-col gap-4">
        <InlineBanner variant="danger">
          This computer&rsquo;s clock is about {Math.abs(minutes)} minutes{" "}
          {minutes > 0 ? "ahead of" : "behind"} the server&rsquo;s. The gateway refuses anything
          signed more than five minutes off its own time, so this console can&rsquo;t tell whether
          its key is still good. Correct the clock and try again. Nothing has been changed.
        </InlineBanner>
        <div className="flex flex-wrap gap-3">
          <Button type="button" onClick={onRetry}>
            Try again
          </Button>
          <Button type="button" variant="ghost" onClick={onPairAgain}>
            Pair this console again
          </Button>
        </div>
      </div>
    </div>
  );
}
