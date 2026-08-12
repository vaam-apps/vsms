// Server component shell (#57). No `useSearchParams`/`nuqs` here (this
// screen has no filters to hold in the URL — a six-role snapshot needs
// none), so unlike `jobs/page.tsx` there is no Suspense boundary to add.

import { WorkersScreen } from "./workers-screen";

export default function WorkersPage() {
  return <WorkersScreen />;
}
