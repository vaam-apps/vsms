// Server component shell (T12). The one thing worth doing server-side
// here: reading `MESSAGE_STREAM_POLL_MS` from `@vsms/env` and handing it
// to the client screen as a prop, so the browser's own poll cadence
// (`messages-screen.tsx`'s `refetchInterval`) stays in lockstep with the
// server's actual `MessageStreamHub` interval without needing a second,
// `NEXT_PUBLIC_*` copy of the same setting.

import { env } from "@vsms/env";
import { Suspense } from "react";
import { MessagesScreen } from "./messages-screen";

export default function MessagesPage() {
  return (
    // `nuqs`'s `useQueryStates` reads `useSearchParams()` internally,
    // which Next.js requires a Suspense boundary around — otherwise every
    // page using it opts the whole route out of static generation with a
    // build-time error ("should be wrapped in a suspense boundary").
    <Suspense fallback={null}>
      <MessagesScreen pollMs={env.MESSAGE_STREAM_POLL_MS} />
    </Suspense>
  );
}
