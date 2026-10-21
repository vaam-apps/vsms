// Dumb component (R6): the Messages list's filters, drawn as the list's own
// toolbar. The state picker, the client reference and the date range on one
// row; the date presets as M3 filter chips under it, selected when the range
// matches. Every value comes in as a prop and goes out through a callback:
// the URL (nuqs) owns the state in `messages-screen.tsx`.

import {
  Button,
  FormField,
  Input,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@vaam-apps/ui";
import { FilterChip } from "@/components/filter-chip";
import { MESSAGE_STATES, MESSAGE_STATUS_META, type MessageState } from "@/components/status";
import type { DatePreset } from "../date-range";

export interface MessagesFiltersProps {
  state: MessageState | null;
  clientRef: string;
  from: string;
  to: string;
  preset: DatePreset | null;
  hasFilters: boolean;
  onStateChange: (state: MessageState | null) => void;
  onClientRefChange: (value: string) => void;
  onFromChange: (value: string) => void;
  onToChange: (value: string) => void;
  onPreset: (preset: DatePreset) => void;
  onClear: () => void;
}

const PRESETS: { value: DatePreset; label: string }[] = [
  { value: "today", label: "Today" },
  { value: "7d", label: "Last 7 days" },
  { value: "30d", label: "Last 30 days" },
];

export function MessagesFilters(props: MessagesFiltersProps) {
  return (
    <>
      <div className="grid grid-cols-2 gap-3 md:grid-cols-[180px_minmax(0,1fr)_160px_160px]">
        <FormField label="State" htmlFor="filter-state">
          <Select
            value={props.state ?? "__all"}
            onValueChange={(value) =>
              props.onStateChange(value === "__all" ? null : (value as MessageState))
            }
          >
            <SelectTrigger id="filter-state">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="__all">All states</SelectItem>
              {MESSAGE_STATES.map((s) => (
                <SelectItem key={s} value={s}>
                  {MESSAGE_STATUS_META[s].label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </FormField>
        <FormField label="Client reference" htmlFor="filter-client-ref">
          <Input
            id="filter-client-ref"
            placeholder="Exact match"
            value={props.clientRef}
            onChange={(e) => props.onClientRefChange(e.target.value)}
          />
        </FormField>
        <FormField label="From" htmlFor="filter-from">
          <Input
            id="filter-from"
            type="date"
            value={props.from}
            max={props.to || undefined}
            onChange={(e) => props.onFromChange(e.target.value)}
          />
        </FormField>
        <FormField label="To" htmlFor="filter-to">
          <Input
            id="filter-to"
            type="date"
            value={props.to}
            min={props.from || undefined}
            onChange={(e) => props.onToChange(e.target.value)}
          />
        </FormField>
      </div>
      <div className="flex flex-wrap items-center gap-2">
        {PRESETS.map((preset) => (
          <FilterChip
            key={preset.value}
            selected={props.preset === preset.value}
            onClick={() => props.onPreset(preset.value)}
          >
            {preset.label}
          </FilterChip>
        ))}
        {props.hasFilters && (
          <Button type="button" variant="ghost" size="sm" onClick={props.onClear}>
            Clear filters
          </Button>
        )}
      </div>
    </>
  );
}
