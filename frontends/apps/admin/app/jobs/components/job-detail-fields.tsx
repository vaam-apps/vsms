// Dumb component (R6): the quick-detail drawer's field list, moved verbatim
// out of `jobs-screen.tsx` (`JobDetailField` included).

import { DetailList, DetailRow, IdDisplay, JobStatusPill, TimestampDisplay } from "@vsms/ui";
import type { JobListItem } from "./jobs-table";

export interface JobDetailFieldsProps {
  job: JobListItem;
}

export function JobDetailFields({ job }: JobDetailFieldsProps) {
  return (
    <DetailList variant="divided">
      <DetailRow variant="divided" label="State">
        {<JobStatusPill state={job.state} />}
      </DetailRow>
      <DetailRow variant="divided" label="Kind">
        {<span className="font-mono">{job.kind}</span>}
      </DetailRow>
      <DetailRow variant="divided" label="Attempts">
        {
          <span className="font-mono">
            {job.attempts}/{job.maxAttempts}
          </span>
        }
      </DetailRow>
      <DetailRow variant="divided" label="Priority">
        {<span className="font-mono">{job.priority}</span>}
      </DetailRow>
      <DetailRow variant="divided" label="Dedupe key">
        {job.dedupeKey != null ? <span className="break-all font-mono">{job.dedupeKey}</span> : "—"}
      </DetailRow>
      <DetailRow variant="divided" label="Last error">
        {job.lastError != null ? (
          <span className="whitespace-pre-wrap break-words font-mono text-caption">
            {job.lastError}
          </span>
        ) : (
          "—"
        )}
      </DetailRow>
      <DetailRow variant="divided" label="Run at">
        {<TimestampDisplay value={job.runAt} />}
      </DetailRow>
      <DetailRow variant="divided" label="Lease owner">
        {job.leaseOwner != null ? (
          <span className="break-all font-mono">{job.leaseOwner}</span>
        ) : (
          "—"
        )}
      </DetailRow>
      <DetailRow variant="divided" label="Lease until">
        {job.leaseUntil != null ? <TimestampDisplay value={job.leaseUntil} /> : "—"}
      </DetailRow>
      <DetailRow variant="divided" label="Started at">
        {job.startedAt != null ? <TimestampDisplay value={job.startedAt} /> : "—"}
      </DetailRow>
      <DetailRow variant="divided" label="Finished at">
        {job.finishedAt != null ? <TimestampDisplay value={job.finishedAt} /> : "—"}
      </DetailRow>
      <DetailRow variant="divided" label="Id">
        {<IdDisplay value={job.id} variant="full" />}
      </DetailRow>
      <DetailRow variant="divided" label="Version">
        {<span className="font-mono">{job.version}</span>}
      </DetailRow>
      <DetailRow variant="divided" label="Created">
        {<TimestampDisplay value={job.createdAt} />}
      </DetailRow>
      <DetailRow variant="divided" label="Updated">
        {<TimestampDisplay value={job.updatedAt} />}
      </DetailRow>
    </DetailList>
  );
}
