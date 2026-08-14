// Dumb component (R6): a single-line error banner. Route-local rather than
// shared — the identical markup is duplicated across several screens'
// smart components today (found while extracting this one; see this PR's
// own description), a real candidate for a future shared `@vsms/ui`
// primitive, but not added there here to avoid colliding with the other
// screen-owning agents doing the same extraction in parallel.

export interface ErrorBannerProps {
  message: string;
}

export function ErrorBanner({ message }: ErrorBannerProps) {
  return (
    <div className="rounded-sm border border-state-danger-border bg-state-danger-bg px-3 py-2 text-caption text-state-danger-fg">
      {message}
    </div>
  );
}
