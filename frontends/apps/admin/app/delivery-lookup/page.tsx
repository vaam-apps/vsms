// Server component shell. `DeliveryLookupScreen` reads the address's query
// string through `nuqs`, which Next requires a `Suspense` boundary around.

import { RouteSkeleton } from "@vaam-apps/ui";
import { Suspense } from "react";
import { DeliveryLookupScreen } from "./delivery-lookup-screen";

export default function DeliveryLookupPage() {
  return (
    <Suspense fallback={<RouteSkeleton />}>
      <DeliveryLookupScreen />
    </Suspense>
  );
}
