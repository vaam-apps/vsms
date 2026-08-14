// Dumb component (R6): the quick-detail drawer's field list, moved verbatim
// out of `workers-screen.tsx` (`WorkerDetailField` included).

import { TimestampDisplay } from "@vsms/ui";
import type { ReactNode } from "react";
import { roleLabel } from "../role-labels";
import { StatusIndicator, type WorkerLockInfo } from "./workers-table";

function WorkerDetailField({ label, value }: { label: string; value: ReactNode }) {
  return (
    <div className="flex flex-col gap-0.5 border-edge-subtle border-b py-2 last:border-b-0">
      <dt className="text-caption text-subtle-foreground">{label}</dt>
      <dd className="text-body text-foreground">{value}</dd>
    </div>
  );
}

export interface WorkerDetailFieldsProps {
  lock: WorkerLockInfo;
}

export function WorkerDetailFields({ lock }: WorkerDetailFieldsProps) {
  return (
    <dl className="flex flex-col">
      <WorkerDetailField label="Role" value={<span className="font-mono">{lock.role}</span>} />
      <WorkerDetailField label="Status" value={<StatusIndicator lock={lock} />} />
      <WorkerDetailField
        label="Cardinality"
        value={lock.singleton ? "Singleton (one lease at a time)" : "Scale-to-N (no lease)"}
      />
      <WorkerDetailField
        label="Worker id"
        value={
          lock.workerId != null ? <span className="break-all font-mono">{lock.workerId}</span> : "—"
        }
      />
      <WorkerDetailField
        label="Postgres pid"
        value={lock.pid != null ? <span className="font-mono">{lock.pid}</span> : "—"}
      />
      <WorkerDetailField
        label="Held since"
        value={lock.heldSince != null ? <TimestampDisplay value={lock.heldSince} /> : "—"}
      />
    </dl>
  );
}

export function workerDetailTitle(lock: WorkerLockInfo | null): string {
  return lock != null ? roleLabel(lock.role) : "Role";
}

export function workerDetailDescription(lock: WorkerLockInfo | null): string | undefined {
  if (lock === null) return undefined;
  return lock.singleton
    ? "Singleton role — one lease at a time."
    : "Scale-to-N role — never takes this lock.";
}
