// Dumb component (R6): the "Provider balance" card. Static content, no
// props — `poll_balance` (§7.5) was never built, so there is no number to
// pass in. Moved verbatim out of `dashboard-screen.tsx`.

import { Card, CardBody, CardHeader } from "@vsms/ui";

export function BalanceCard() {
  return (
    <Card>
      <CardHeader title="Provider balance" />
      <CardBody>
        <p className="text-caption text-muted-foreground">
          Not available. <span className="font-mono text-foreground">poll_balance</span> (§7.5) was
          never built — there is no source of truth for provider balance anywhere in this system
          yet. This card intentionally shows no number rather than a fabricated one.
        </p>
      </CardBody>
    </Card>
  );
}
