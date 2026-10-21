// Dumb component (R6, route-local): the Jobs list's toolbar. One chip per
// job state (plus All) instead of a dropdown, because there are only five
// states and the operator's question is usually exactly one of them
// ("anything dead?"); the free-text kind filter sits beside them.

import { Button, Input } from "@vaam-apps/ui";
import { Search } from "lucide-react";
import { ChipGroup } from "@/components/chip-group";
import { FilterChip } from "@/components/filter-chip";
import { JOB_STATES, JOB_STATUS_META, type JobState } from "@/components/status";

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
    <div className="flex flex-col gap-3 lg:flex-row lg:items-center lg:justify-between">
      <ChipGroup label="Job state">
        <FilterChip selected={state === null} onClick={() => onStateChange(null)}>
          All
        </FilterChip>
        {JOB_STATES.map((s) => (
          <FilterChip key={s} selected={state === s} onClick={() => onStateChange(s)}>
            {JOB_STATUS_META[s].label}
          </FilterChip>
        ))}
      </ChipGroup>
      <div className="flex items-center gap-2">
        <div className="relative w-full lg:w-64">
          <Search
            size={16}
            aria-hidden="true"
            className="pointer-events-none absolute top-1/2 left-3 z-10 -translate-y-1/2 text-muted-foreground"
          />
          <Input
            id="filter-kind"
            aria-label="Job kind"
            placeholder="Filter by kind"
            className="w-full pl-9"
            value={kind}
            onChange={(e) => onKindChange(e.target.value)}
          />
        </div>
        {hasFilters && (
          <Button type="button" variant="ghost" size="sm" onClick={onClear}>
            Clear
          </Button>
        )}
      </div>
    </div>
  );
}
