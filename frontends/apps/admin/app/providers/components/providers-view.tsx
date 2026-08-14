// Dumb component (R6): the Providers screen's own top-level layout — a
// single `flex flex-col gap-6` column, moved verbatim out of
// `providers-screen.tsx`.
//
// One fix made in the same move, not just a relocation: the original wrapper
// was `<main className="mx-auto flex max-w-[1400px] ... px-4 py-6 ...">` —
// a second `<main>` nested inside `ConsoleShell`'s own `<main>` (which
// already applies `mx-auto max-w-[1400px] px-4 py-6 lg:px-8 lg:py-10` around
// every route, per `console-shell.tsx`). That's invalid HTML (two `<main>`
// landmarks) and double padding — the exact bug `dashboard-screen.tsx`'s own
// "Console-redesign Phase 2" comment already documents having fixed there;
// this screen just hadn't received the same cleanup yet. Fixed here to a
// plain `<div>`, matching `DashboardView`'s own wrapper.

import type { ReactNode } from "react";
import { ErrorBanner } from "./error-banner";
import { ProvidersHeader } from "./providers-header";
import { ScopeBanner } from "./scope-banner";

export interface ProvidersViewProps {
  errorMessage: string | null;
  table: ReactNode;
  quickDetail: ReactNode;
  editDrawer: ReactNode;
}

export function ProvidersView({
  errorMessage,
  table,
  quickDetail,
  editDrawer,
}: ProvidersViewProps) {
  return (
    <div className="flex flex-col gap-6">
      <ProvidersHeader />
      <ScopeBanner />
      {errorMessage != null && (
        <ErrorBanner message={`Could not read providers: ${errorMessage}`} />
      )}
      {table}
      {quickDetail}
      {editDrawer}
    </div>
  );
}
