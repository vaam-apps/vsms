// Server component shell (#52). No filters/`nuqs` state to hold in the URL
// — a small app roster needs none, same reasoning `providers/page.tsx`
// gives.

import { AppsScreen } from "./apps-screen";

export default function AppsPage() {
  return <AppsScreen />;
}
