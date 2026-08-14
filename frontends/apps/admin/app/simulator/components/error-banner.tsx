// Dumb component (R6): a single-line error banner. Route-local rather than
// shared, same reasoning as `dashboard/components/error-banner.tsx` and
// `providers/components/error-banner.tsx` — real duplication, flagged in
// this PR's own description rather than hoisted into `@vsms/ui` to avoid
// colliding with the other screen-owning agents doing the same extraction
// in parallel. The class string that used to be duplicated here is gone —
// `InlineBanner` (`r6-factorize-base`) now owns it — but the wrapper stays,
// still route-local for the same collision-avoidance reason.

import { InlineBanner } from "@vsms/ui";

export interface ErrorBannerProps {
  message: string;
}

export function ErrorBanner({ message }: ErrorBannerProps) {
  return <InlineBanner variant="danger">{message}</InlineBanner>;
}
