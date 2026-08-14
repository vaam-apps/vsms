// Dumb — route-local to messages (R6). Static copy; see `messages-
// screen.tsx`'s own module doc ("Why the visible scope banner") for why
// this exists and what it's explaining. No props: this is fixed text
// about a fixed, deliberate behaviour, not data.

export function CrossAppScopeBanner() {
  return (
    <div className="rounded-sm border border-edge bg-surface-2 px-3 py-2 text-caption text-muted-foreground">
      This list spans <span className="font-mono text-foreground">every app</span> in this
      deployment — you're reading it as yourself, not as a single app's service account. Live
      updates are narrower: they only arrive for this console's own app, so a row belonging to
      another app won't update on screen until you refresh. Not a filter and not a bug; see
      `messages-screen.tsx`'s own module doc.
    </div>
  );
}
