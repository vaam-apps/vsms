// Dumb view: the screen title plus the "genuinely read-only" note. Static
// copy — no props needed.

import { InlineBanner, ScreenHeader } from "@vsms/ui";

export function AuditLogHeader() {
  return (
    <>
      <ScreenHeader
        title="Audit log"
        description="Every audited write in this system, and whether the tamper-evidence chain over it still verifies. Read-only — see this screen's own note below."
      />

      <InlineBanner variant="neutral">
        This view is genuinely read-only, not just missing an edit button — no role, including{" "}
        <span className="font-mono text-foreground">system</span>, can write an audit anchor through
        any path this codebase exposes.
      </InlineBanner>
    </>
  );
}
