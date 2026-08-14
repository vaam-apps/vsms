// Dumb — route-local to messages (R6). The filter row: state, client
// reference, a from/to date range, and the three quick-range buttons.
// Every value and every change is handed in or reported out via props —
// this component holds no URL state and doesn't know `nuqs` exists.

import {
  Button,
  Input,
  Label,
  MESSAGE_STATES,
  MESSAGE_STATUS_META,
  type MessageState,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@vsms/ui";

export interface MessagesFiltersProps {
  state: MessageState | null;
  clientRef: string;
  from: string;
  to: string;
  hasFilters: boolean;
  onStateChange: (state: MessageState | null) => void;
  onClientRefChange: (value: string) => void;
  onFromChange: (value: string) => void;
  onToChange: (value: string) => void;
  onSelectToday: () => void;
  onSelectLast7Days: () => void;
  onSelectLast30Days: () => void;
  onClear: () => void;
}

export function MessagesFilters({
  state,
  clientRef,
  from,
  to,
  hasFilters,
  onStateChange,
  onClientRefChange,
  onFromChange,
  onToChange,
  onSelectToday,
  onSelectLast7Days,
  onSelectLast30Days,
  onClear,
}: MessagesFiltersProps) {
  return (
    <div className="flex flex-col flex-wrap gap-4 sm:flex-row sm:items-end">
      <div className="flex flex-col gap-1.5 sm:w-[180px]">
        <Label htmlFor="filter-state">State</Label>
        <Select
          value={state ?? "__all"}
          onValueChange={(value) =>
            onStateChange(value === "__all" ? null : (value as MessageState))
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
      </div>

      <div className="flex flex-col gap-1.5 sm:w-[200px]">
        <Label htmlFor="filter-client-ref">Client reference</Label>
        <Input
          id="filter-client-ref"
          placeholder="exact match"
          value={clientRef}
          onChange={(e) => onClientRefChange(e.target.value)}
        />
      </div>

      <div className="flex gap-4">
        <div className="flex flex-1 flex-col gap-1.5 sm:w-[160px] sm:flex-none">
          <Label htmlFor="filter-from">From</Label>
          <Input
            id="filter-from"
            type="date"
            value={from}
            max={to || undefined}
            onChange={(e) => onFromChange(e.target.value)}
          />
        </div>
        <div className="flex flex-1 flex-col gap-1.5 sm:w-[160px] sm:flex-none">
          <Label htmlFor="filter-to">To</Label>
          <Input
            id="filter-to"
            type="date"
            value={to}
            min={from || undefined}
            onChange={(e) => onToChange(e.target.value)}
          />
        </div>
      </div>

      <div className="flex flex-wrap items-center gap-2 sm:pb-0.5">
        <Button type="button" variant="secondary" size="sm" onClick={onSelectToday}>
          Today
        </Button>
        <Button type="button" variant="secondary" size="sm" onClick={onSelectLast7Days}>
          Last 7 days
        </Button>
        <Button type="button" variant="secondary" size="sm" onClick={onSelectLast30Days}>
          Last 30 days
        </Button>
        {hasFilters && (
          <Button type="button" variant="ghost" size="sm" onClick={onClear}>
            Clear filters
          </Button>
        )}
      </div>
    </div>
  );
}
