// Dumb component (R6, route-local): a role's lease status as one chip.
// Moved out of the deleted `workers-table.tsx` when the table became a grid
// of role cards; the wording is unchanged except "scale-to-N — no lock",
// now "Runs on every node", which says what an operator needs to know (it
// is not supposed to hold anything) instead of naming the mechanism.

import type { inferRouterOutputs } from "@trpc/server";
import { StateChip } from "@vaam-apps/ui";
import type { AppRouter } from "@vsms/api";

type RouterOutputs = inferRouterOutputs<AppRouter>;
export type WorkerLockInfo = RouterOutputs["workers"]["locks"]["locks"][number];

export function StatusIndicator({ lock }: { lock: WorkerLockInfo }) {
  if (!lock.singleton) {
    return (
      <StateChip tone="neutral" size="sm">
        Runs on every node
      </StateChip>
    );
  }
  if (lock.held) {
    return (
      <StateChip tone="success" size="sm">
        Held
      </StateChip>
    );
  }
  return (
    <StateChip tone="danger" size="sm">
      Unheld
    </StateChip>
  );
}
