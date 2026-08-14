// Dumb component (R6): a single-line error banner. Route-local rather than
// shared, same reasoning as `dashboard/components/error-banner.tsx` and
// `providers/components/error-banner.tsx` — real duplication, flagged in
// this PR's own description rather than hoisted into `@vsms/ui` to avoid
// colliding with the other screen-owning agents doing the same extraction
// in parallel.

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
