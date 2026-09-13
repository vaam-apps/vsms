// Dumb component (R6): the Route simulator's own top-level layout — a
// single `flex flex-col gap-6` column, moved verbatim out of
// `simulator-screen.tsx`. Composes the smaller dumb components in this
// directory via slots the screen fills in, plus the loading/no-routes
// branching that decides which of them actually render.

import { SkeletonText } from "@vaam-apps/ui";
import type { ReactNode } from "react";
import { ErrorBanner } from "./error-banner";
import { NoRoutesBanner } from "./no-routes-banner";
import { SimulatorHeader } from "./simulator-header";

export interface SimulatorViewProps {
  candidateForm: ReactNode;
  errorMessage: string | null;
  isFetchingFirstResult: boolean;
  noRoutesConfigured: boolean;
  result: ReactNode;
}

export function SimulatorView({
  candidateForm,
  errorMessage,
  isFetchingFirstResult,
  noRoutesConfigured,
  result,
}: SimulatorViewProps) {
  return (
    <div className="mx-auto flex w-full max-w-[1100px] flex-col gap-6">
      <SimulatorHeader />
      {candidateForm}
      {errorMessage != null && <ErrorBanner message={errorMessage} />}
      {/* SkeletonText, not a flat slab: the real result card is a decision
          summary plus a per-route evaluation list, and a paragraph-shaped
          placeholder with a short last line reads as that arriving rather
          than as one indistinct block. `lines={7}` approximates the same
          visual weight the previous `h-40` slab held. */}
      {isFetchingFirstResult && <SkeletonText lines={7} />}
      {noRoutesConfigured && <NoRoutesBanner />}
      {result}
    </div>
  );
}
