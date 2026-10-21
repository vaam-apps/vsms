// Dumb component (R6, route-local): the Providers screen's layout. The
// header, a danger banner when the list cannot be read, the provider cards,
// a caption on who may save, and the two drawers.

import { InlineBanner, ScreenStack } from "@vaam-apps/ui";
import type { ReactNode } from "react";
import { Caption } from "@/components/caption";
import { PageHeader } from "@/components/page-header";

export interface ProvidersViewProps {
  errorMessage: string | null;
  cards: ReactNode;
  quickDetail: ReactNode;
  editDrawer: ReactNode;
}

export function ProvidersView({
  errorMessage,
  cards,
  quickDetail,
  editDrawer,
}: ProvidersViewProps) {
  return (
    <ScreenStack>
      <PageHeader
        title="Providers"
        description="Every configured SMS provider, with its capacity, cost and current state."
      />
      {errorMessage != null && (
        <InlineBanner variant="danger">Could not read providers: {errorMessage}</InlineBanner>
      )}
      {cards}
      <Caption>
        Edits are saved as you. Saving needs the <span className="font-mono">provider:update</span>{" "}
        permission, which owner, admin and operator hold; a change someone saved first is reported,
        never overwritten.
      </Caption>
      {quickDetail}
      {editDrawer}
    </ScreenStack>
  );
}
