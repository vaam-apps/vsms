// Dumb component (R6): the closing footnote pointing at Prometheus for
// outbox-age/poison-row alerting. Static content, no props. Moved verbatim
// out of `dashboard-screen.tsx`; its `text-caption text-subtle-foreground`
// wrapper is `InlineBanner variant="plain"`'s own class pair, so it renders
// through that primitive rather than a second hand-rolled copy of it.

import { InlineBanner } from "@vsms/ui";

export function OutboxFootnote() {
  return (
    <InlineBanner variant="plain">
      Outbox age and poison-row alerting live in Prometheus, not here — see{" "}
      <span className="font-mono">deploy/prometheus/alerts.yml</span>. This screen's{" "}
      <span className="font-mono text-foreground">Outbox depth</span> tile is a genuine current
      count of <span className="font-mono text-foreground">WebhookAttempt</span> rows, a different
      table from the framework's own internal event outbox those alerts describe.
    </InlineBanner>
  );
}
