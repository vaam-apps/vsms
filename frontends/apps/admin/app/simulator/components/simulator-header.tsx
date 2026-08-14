// Dumb component (R6): the Route simulator's own title block. Markup moved
// verbatim out of `simulator-screen.tsx`.
//
// The title/description pair is `@vsms/ui`'s `ScreenHeader` (see
// `messages-header.tsx`'s own note on why it's used directly here rather
// than hand-rolled, and why the divider stays a thin route-local wrapper).

import { ScreenHeader } from "@vsms/ui";

export function SimulatorHeader() {
  return (
    <div className="border-edge border-b pb-6">
      <ScreenHeader
        title="Route simulator"
        description="Given a recipient, message class, and app, which route wins and why — without sending anything. Renders the real routing engine's own decision."
      />
    </div>
  );
}
