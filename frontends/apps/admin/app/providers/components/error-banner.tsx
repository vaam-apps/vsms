// Dumb component (R6): a single-line error banner. Route-local rather than
// a bare `InlineBanner` at the call site because `providers-view.tsx` wants
// a named, single-purpose wrapper for "the list read error" the way the
// `quickDetail`/`editDrawer`/`table` slots already are — same reasoning
// `dashboard/components/error-banner.tsx` gives for its own equivalent.

import { InlineBanner } from "@vsms/ui";

export interface ErrorBannerProps {
  message: string;
}

export function ErrorBanner({ message }: ErrorBannerProps) {
  return <InlineBanner variant="danger">{message}</InlineBanner>;
}
