// Dumb component (R6): the app-scope explanation banner. Markup and copy
// moved verbatim out of `dashboard-screen.tsx` — see that screen's own
// module doc ("Why the scope banner") for the #211 background this text
// refers to. `appScoped` decides which of the two paragraphs renders; the
// screen derives it from `summary.appId`'s presence/absence, matching
// `messages-screen.tsx`'s own precedent.

export interface ScopeBannerProps {
  appScoped: boolean;
}

export function ScopeBanner({ appScoped }: ScopeBannerProps) {
  return (
    <div className="rounded-sm border border-edge bg-surface-2 px-3 py-2 text-caption text-muted-foreground">
      {!appScoped ? (
        <>
          You're reading this as yourself — message- and webhook-based tiles below cover{" "}
          <span className="font-mono text-foreground">every app</span> in this deployment, not one.{" "}
        </>
      ) : (
        <>
          Message- and webhook-based tiles below are scoped to{" "}
          <span className="font-mono text-foreground">this app only</span> — the console's own
          service-account token can only read the one app it belongs to.{" "}
        </>
      )}
      <span className="font-mono text-foreground">Job backlog</span> is always system-wide, because{" "}
      <span className="font-mono text-foreground">Job</span> has no app boundary to scope by.
      Neither is a filter, and neither is a bug.
    </div>
  );
}
