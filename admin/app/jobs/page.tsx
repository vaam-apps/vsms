// Server component shell (#56), same shape `messages/page.tsx` uses:
// `useQueryStates` needs a Suspense boundary around it (Next.js requires
// this for `useSearchParams()`), so the client screen is wrapped here
// rather than opting the whole route out of static generation.

import { Suspense } from "react";
import { JobsScreen } from "./jobs-screen";

export default function JobsPage() {
  return (
    <Suspense fallback={null}>
      <JobsScreen />
    </Suspense>
  );
}
