// Server component shell (#51, T13). Pure composition: the composer's own
// data fetching, mutations and form state all live in `ComposerScreen`
// (R6). It was the console's landing route until the dashboard took `/`.

import { ComposerScreen } from "./composer-screen";

export default function ComposerPage() {
  return <ComposerScreen />;
}
