// Server component shell (#51, T13) — the console's landing route. Pure
// composition: the composer's own data fetching, mutations and form state
// all live in `ComposerScreen` (R6, AGENTS.md).

import { ComposerScreen } from "./composer/composer-screen";

export default function ComposerPage() {
  return <ComposerScreen />;
}
