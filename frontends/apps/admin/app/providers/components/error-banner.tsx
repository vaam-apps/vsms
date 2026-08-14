// Dumb component (R6): a single-line error banner. Route-local rather than
// shared for the same reason `dashboard/components/error-banner.tsx`
// gives — real duplication, not created as a shared primitive here to
// avoid colliding with the other screen-owning agents extracting the
// identical markup from their own screens in parallel.

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
