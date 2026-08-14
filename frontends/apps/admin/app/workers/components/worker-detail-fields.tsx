// Dumb component (R6): the quick-detail drawer's field list, moved verbatim
// out of `workers-screen.tsx` (`WorkerDetailField` included).

import { DetailList, DetailRow, TimestampDisplay } from "@vsms/ui";
import { roleLabel } from "../role-labels";
import { StatusIndicator, type WorkerLockInfo } from "./workers-table";

export interface WorkerDetailFieldsProps {
  lock: WorkerLockInfo;
}

export function WorkerDetailFields({ lock }: WorkerDetailFieldsProps) {
  return (
    <DetailList variant="divided">
      <DetailRow variant="divided" label="Role">
        {<span className="font-mono">{lock.role}</span>}
      </DetailRow>
      <DetailRow variant="divided" label="Status">
        {<StatusIndicator lock={lock} />}
      </DetailRow>
      <DetailRow variant="divided" label="Cardinality">
        {lock.singleton ? "Singleton (one lease at a time)" : "Scale-to-N (no lease)"}
      </DetailRow>
      <DetailRow variant="divided" label="Worker id">
        {lock.workerId != null ? <span className="break-all font-mono">{lock.workerId}</span> : "—"}
      </DetailRow>
      <DetailRow variant="divided" label="Postgres pid">
        {lock.pid != null ? <span className="font-mono">{lock.pid}</span> : "—"}
      </DetailRow>
      <DetailRow variant="divided" label="Held since">
        {lock.heldSince != null ? <TimestampDisplay value={lock.heldSince} /> : "—"}
      </DetailRow>
    </DetailList>
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
