// Server component shell (#54). No filters/`nuqs` state to hold in the URL
// — a small, unpaged provider list needs none, same reasoning
// `workers/page.tsx` gives.

import { ProvidersScreen } from "./providers-screen";

export default function ProvidersPage() {
  return <ProvidersScreen />;
}
