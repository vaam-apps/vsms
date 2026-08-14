// Server component shell (#53). No filters/`nuqs` state to hold in the URL
// — a small, unpaged sender-id list needs none, same reasoning
// `providers/page.tsx` gives.

import { SenderIdsScreen } from "./sender-ids-screen";

export default function SenderIdsPage() {
  return <SenderIdsScreen />;
}
