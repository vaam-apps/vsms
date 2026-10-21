// Dumb component (R6, route-local): the six worker roles as cards.
//
// There are always exactly six roles, so the screen used to be a six-row
// table whose Node and Pid columns vanished below `md`, leaving a phone with
// a role name and a chip. A card per role keeps the node and the lease age
// visible at every width, and reads at a glance: a singleton role's card is
// outlined in the danger hue when nobody holds its lease. A card is a
// button; it opens the same detail drawer the row did.

import { Skeleton, TimestampDisplay } from "@vaam-apps/ui";
import { ChevronRight } from "lucide-react";
import { ROLE_PURPOSES, roleLabel } from "../role-labels";
import { StatusIndicator, type WorkerLockInfo } from "./worker-status";

export interface WorkerRoleGridProps {
  locks: WorkerLockInfo[];
  isLoading: boolean;
  onOpen: (lock: WorkerLockInfo) => void;
}

export function WorkerRoleGrid({ locks, isLoading, onOpen }: WorkerRoleGridProps) {
  if (isLoading) {
    return (
      <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-3">
        {["a", "b", "c", "d", "e", "f"].map((key) => (
          <Skeleton key={key} className="h-36 rounded-[20px]" />
        ))}
      </div>
    );
  }
  return (
    <ul className="grid gap-3 sm:grid-cols-2 xl:grid-cols-3">
      {locks.map((lock) => {
        return (
          <li key={lock.role}>
            <button
              type="button"
              onClick={() => onOpen(lock)}
              aria-label={`${roleLabel(lock.role)} role details`}
              className="flex h-full w-full flex-col gap-3 rounded-[20px] border border-edge-subtle bg-base-100 p-5 text-left transition-colors hover:bg-surface-1 focus-visible:outline-2 focus-visible:outline-primary"
            >
              <span className="flex items-start justify-between gap-3">
                <span className="flex flex-col gap-1">
                  <span className="text-title-medium text-foreground">{roleLabel(lock.role)}</span>
                  <span className="text-body-small text-muted-foreground">
                    {ROLE_PURPOSES[lock.role] ?? ""}
                  </span>
                </span>
                <ChevronRight
                  size={18}
                  aria-hidden="true"
                  className="mt-1 shrink-0 text-subtle-foreground"
                />
              </span>
              <span className="mt-auto flex flex-col gap-3">
                <span className="flex flex-wrap items-center gap-2">
                  <StatusIndicator lock={lock} />
                </span>
                <span className="flex flex-col gap-0.5 text-body-small text-muted-foreground">
                  <span className="truncate font-mono text-foreground">
                    {lock.workerId ?? (lock.singleton ? "No node" : "Every running worker")}
                  </span>
                  {lock.heldSince != null && (
                    <span>
                      since <TimestampDisplay value={lock.heldSince} />
                    </span>
                  )}
                </span>
              </span>
            </button>
          </li>
        );
      })}
    </ul>
  );
}
