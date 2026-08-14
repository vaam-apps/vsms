// Dumb component (R6): the #211 own-credential explainer banner. Static
// content, no props — moved verbatim out of `providers-screen.tsx`.

export function ScopeBanner() {
  return (
    <div className="rounded-sm border border-edge bg-surface-2 px-3 py-2 text-caption text-muted-foreground">
      Reads and writes both act as you, not as a shared service account — Save requires your own
      role to carry <span className="font-mono text-foreground">provider:update</span> (owner,
      admin, and operator all do by default). A role without it, or a stale edit someone else
      already saved, surfaces as a real error here rather than silently failing.
    </div>
  );
}
