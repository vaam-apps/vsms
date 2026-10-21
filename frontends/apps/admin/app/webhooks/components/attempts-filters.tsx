// Dumb component (R6, route-local): the delivery-attempts list's toolbar.
// The endpoint is a dropdown (there can be many, and their URLs are long);
// the state is a row of chips (five states, and "anything dead?" is the
// usual question). The section heading, the refresh cadence, the window
// note and the error moved out to the screen's own section header, list
// footer and banner.

import {
  Button,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@vaam-apps/ui";
import { ChipGroup } from "@/components/chip-group";
import { FilterChip } from "@/components/filter-chip";
import { ATTEMPT_STATES, ATTEMPT_STATUS_META, type AttemptState } from "@/components/status";
import type { EndpointListItem } from "../webhook-domain";

export function AttemptsFilters({
  endpoints,
  endpointId,
  state,
  onEndpointIdChange,
  onStateChange,
  onClearFilters,
}: {
  endpoints: EndpointListItem[] | undefined;
  endpointId: string | null;
  state: AttemptState | null;
  onEndpointIdChange: (value: string | null) => void;
  onStateChange: (value: AttemptState | null) => void;
  onClearFilters: () => void;
}) {
  return (
    <div className="flex flex-col gap-3 lg:flex-row lg:items-center lg:justify-between">
      <ChipGroup label="Attempt state">
        <FilterChip selected={state === null} onClick={() => onStateChange(null)}>
          All
        </FilterChip>
        {ATTEMPT_STATES.map((s) => (
          <FilterChip key={s} selected={state === s} onClick={() => onStateChange(s)}>
            {ATTEMPT_STATUS_META[s].label}
          </FilterChip>
        ))}
      </ChipGroup>
      <div className="flex items-center gap-2">
        <Select
          value={endpointId ?? "__all"}
          onValueChange={(value) => onEndpointIdChange(value === "__all" ? null : value)}
        >
          <SelectTrigger aria-label="Endpoint" className="w-full lg:w-72">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="__all">All endpoints</SelectItem>
            {endpoints?.map((endpoint) => (
              <SelectItem key={endpoint.id} value={endpoint.id}>
                {endpoint.url}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        {(endpointId !== null || state !== null) && (
          <Button type="button" variant="ghost" size="sm" onClick={onClearFilters}>
            Clear
          </Button>
        )}
      </div>
    </div>
  );
}
