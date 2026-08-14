// Dumb component (R6): the #211 own-credential explainer banner. Static
// content, no props — moved verbatim out of `providers-screen.tsx`.

import { InlineBanner } from "@vsms/ui";

export function ScopeBanner() {
  return (
    <InlineBanner variant="neutral">
      Reads and writes both act as you, not as a shared service account — Save requires your own
      role to carry <span className="font-mono text-foreground">provider:update</span> (owner,
      admin, and operator all do by default). A role without it, or a stale edit someone else
      already saved, surfaces as a real error here rather than silently failing.
    </InlineBanner>
  );
}
