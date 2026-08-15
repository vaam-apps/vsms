// Server component shell (#58). `UsersScreen` owns `?tab=`/`?panel=` nuqs
// state (docs/design/console-redesign.md §3/D14), which needs a `Suspense`
// boundary around it the same shape `jobs/page.tsx`/`messages/page.tsx`
// already establish for their own `useQueryStates` calls (Next.js requires
// this for `useSearchParams()`).

import { RouteSkeleton } from "@vsms/ui";
import { Suspense } from "react";
import { UsersScreen } from "./users-screen";

export default function UsersPage() {
  // #308: `fallback={null}` used to sit here — see `RouteSkeleton`'s own
  // doc comment for why that made a slow-to-reveal boundary
  // indistinguishable from a broken one. Mitigation, not a fix for the
  // underlying rAF-gated reveal.
  return (
    <Suspense fallback={<RouteSkeleton withFilterBar={false} />}>
      <UsersScreen />
    </Suspense>
  );
}
