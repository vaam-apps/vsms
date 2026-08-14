// Dumb component (R6): the State/Kind filter row, moved verbatim out of
// `jobs-screen.tsx`. Owns no state of its own — `state`/`kind` are the
// smart screen's URL state, echoed back here for rendering.

import {
  Button,
  FormField,
  Input,
  JOB_STATES,
  JOB_STATUS_META,
  type JobState,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@vsms/ui";

export interface JobFiltersBarProps {
  state: JobState | null;
  kind: string;
  hasFilters: boolean;
  onStateChange: (state: JobState | null) => void;
  onKindChange: (kind: string) => void;
  onClear: () => void;
}

export function JobFiltersBar({
  state,
  kind,
  hasFilters,
  onStateChange,
  onKindChange,
  onClear,
}: JobFiltersBarProps) {
  return (
    <div className="flex flex-wrap items-end gap-4">
      <FormField label="State" htmlFor="filter-state">
        <Select
          value={state ?? "__all"}
          onValueChange={(value) => onStateChange(value === "__all" ? null : (value as JobState))}
        >
          <SelectTrigger id="filter-state" className="w-[180px]">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="__all">All states</SelectItem>
            {JOB_STATES.map((s) => (
              <SelectItem key={s} value={s}>
                {JOB_STATUS_META[s].label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </FormField>

      <FormField label="Kind" htmlFor="filter-kind">
        <Input
          id="filter-kind"
          placeholder="e.g. expire_stale"
          className="w-[200px]"
          value={kind}
          onChange={(e) => onKindChange(e.target.value)}
        />
      </FormField>

      {hasFilters && (
        <Button type="button" variant="ghost" size="sm" onClick={onClear}>
          Clear filters
        </Button>
      )}
    </div>
  );
}
