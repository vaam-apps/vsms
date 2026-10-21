// Dumb component (R6, route-local): the Routes screen's notices — the
// no-routes alarm and a list read failure. The "New route" button moved
// into the page header's actions, and the long note on who may write moved
// to a caption under the table (`routes-screen.tsx`).

import { InlineBanner } from "@vaam-apps/ui";

export function RouteNotices({
  isEmpty,
  listErrorMessage,
}: {
  isEmpty: boolean;
  listErrorMessage?: string | undefined;
}) {
  return (
    <>
      {isEmpty && (
        <InlineBanner variant="danger">
          No routes are configured, so every message is refused. Add at least one enabled route
          before anything can be dispatched.
        </InlineBanner>
      )}

      {listErrorMessage != null && (
        <InlineBanner variant="danger">Could not read routes: {listErrorMessage}</InlineBanner>
      )}
    </>
  );
}
