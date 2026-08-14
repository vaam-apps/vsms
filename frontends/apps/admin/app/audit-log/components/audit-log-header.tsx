// Dumb view: the screen title plus the "genuinely read-only" note. Static
// copy — no props needed.

export function AuditLogHeader() {
  return (
    <>
      <div className="border-edge border-b pb-6">
        <h1 className="font-medium text-foreground text-title">Audit log</h1>
        <p className="mt-1 max-w-xl text-body text-muted-foreground">
          Every audited write in this system, and whether the tamper-evidence chain over it still
          verifies. Read-only — see this screen&apos;s own note below.
        </p>
      </div>

      <div className="rounded-sm border border-edge bg-surface-2 px-3 py-2 text-caption text-muted-foreground">
        This view is genuinely read-only, not just missing an edit button — no role, including{" "}
        <span className="font-mono text-foreground">system</span>, can write an audit anchor through
        any path this codebase exposes.
      </div>
    </>
  );
}
