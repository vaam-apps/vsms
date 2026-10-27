// Dumb component (R6): what the console shows while it finds out whether this
// browser is paired, and while it sends the person to pair it.

import { Spinner } from "@vaam-apps/ui";

export function GateSplash() {
  return (
    <div
      role="status"
      aria-live="polite"
      className="flex min-h-dvh items-center justify-center gap-3 text-body-medium text-muted-foreground"
    >
      <Spinner />
      <span>Checking this console&rsquo;s key&hellip;</span>
    </div>
  );
}
