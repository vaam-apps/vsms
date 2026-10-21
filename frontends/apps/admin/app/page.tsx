// The console's landing route is the dashboard: an operator opens the
// console to see whether traffic is flowing before doing anything else. The
// composer moved to `/composer`; `/dashboard` redirects here.
//
// No `<Suspense>` boundary, on purpose: `DashboardScreen` reads no search
// params, and a boundary's reveal waits on `requestAnimationFrame`, which a
// background tab never runs (#308, see `dashboard/page.tsx`'s history).
//
// `DASHBOARD_REFETCH_INTERVAL_MS` is read here, server-side, and handed down
// as a prop (R6: a tuning value lives in `@vsms/env`).

import { env } from "@vsms/env";
import { DashboardScreen } from "./dashboard/dashboard-screen";

export default function DashboardPage() {
  return <DashboardScreen refetchIntervalMs={env.DASHBOARD_REFETCH_INTERVAL_MS} />;
}
