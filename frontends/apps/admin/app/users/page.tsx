// Server component shell (#58). `UsersScreen` owns `?tab=`/`?panel=` nuqs
// state (docs/design/console-redesign.md §3/D14), which needs a `Suspense`
// boundary around it the same shape `jobs/page.tsx`/`messages/page.tsx`
// already establish for their own `useQueryStates` calls (Next.js requires
// this for `useSearchParams()`).

import { Suspense } from "react";
import { UsersScreen } from "./users-screen";

export default function UsersPage() {
  return (
    <Suspense fallback={null}>
      <UsersScreen />
    </Suspense>
  );
}
