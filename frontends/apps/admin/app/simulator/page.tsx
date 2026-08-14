// Server component shell (#54). No `nuqs`/URL state — a simulation is a
// point-in-time query against live data, not something worth bookmarking
// (the `Route`/`Provider` rows it reads can change between visits).

import { SimulatorScreen } from "./simulator-screen";

export default function SimulatorPage() {
  return <SimulatorScreen />;
}
