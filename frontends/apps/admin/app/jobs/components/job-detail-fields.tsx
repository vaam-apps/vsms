// Dumb component (R6): the quick-detail drawer's field list, moved verbatim
// out of `jobs-screen.tsx` (`JobDetailField` included).

import { IdDisplay, JobStatusPill, TimestampDisplay } from "@vsms/ui";
import type { ReactNode } from "react";
import type { JobListItem } from "./jobs-table";

function JobDetailField({ label, value }: { label: string; value: ReactNode }) {
  return (
    <div className="flex flex-col gap-0.5 border-edge-subtle border-b py-2 last:border-b-0">
      <dt className="text-caption text-subtle-foreground">{label}</dt>
      <dd className="text-body text-foreground">{value}</dd>
    </div>
  );
}

export interface JobDetailFieldsProps {
  job: JobListItem;
}

export function JobDetailFields({ job }: JobDetailFieldsProps) {
  return (
    <dl className="flex flex-col">
      <JobDetailField label="State" value={<JobStatusPill state={job.state} />} />
      <JobDetailField label="Kind" value={<span className="font-mono">{job.kind}</span>} />
      <JobDetailField
        label="Attempts"
        value={
          <span className="font-mono">
            {job.attempts}/{job.maxAttempts}
          </span>
        }
      />
      <JobDetailField label="Priority" value={<span className="font-mono">{job.priority}</span>} />
      <JobDetailField
        label="Dedupe key"
        value={
          job.dedupeKey != null ? <span className="break-all font-mono">{job.dedupeKey}</span> : "—"
        }
      />
      <JobDetailField
        label="Last error"
        value={
          job.lastError != null ? (
            <span className="whitespace-pre-wrap break-words font-mono text-caption">
              {job.lastError}
            </span>
          ) : (
            "—"
          )
        }
      />
      <JobDetailField label="Run at" value={<TimestampDisplay value={job.runAt} />} />
      <JobDetailField
        label="Lease owner"
        value={
          job.leaseOwner != null ? (
            <span className="break-all font-mono">{job.leaseOwner}</span>
          ) : (
            "—"
          )
        }
      />
      <JobDetailField
        label="Lease until"
        value={job.leaseUntil != null ? <TimestampDisplay value={job.leaseUntil} /> : "—"}
      />
      <JobDetailField
        label="Started at"
        value={job.startedAt != null ? <TimestampDisplay value={job.startedAt} /> : "—"}
      />
      <JobDetailField
        label="Finished at"
        value={job.finishedAt != null ? <TimestampDisplay value={job.finishedAt} /> : "—"}
      />
      <JobDetailField label="Id" value={<IdDisplay value={job.id} variant="full" />} />
      <JobDetailField label="Version" value={<span className="font-mono">{job.version}</span>} />
      <JobDetailField label="Created" value={<TimestampDisplay value={job.createdAt} />} />
      <JobDetailField label="Updated" value={<TimestampDisplay value={job.updatedAt} />} />
    </dl>
  );
}
