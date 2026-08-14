import {
  ATTEMPT_STATES,
  type AttemptState,
  Button,
  Label,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@vsms/ui";
import type { EndpointListItem } from "../webhook-domain";

// Dumb (R6): the delivery-attempts section heading, its refresh cadence
// caption, the endpoint/state filter row with a "Clear filters" action,
// and the truncated/read-error banners.
export function AttemptsToolbar({
  refetchIntervalMs,
  endpoints,
  endpointId,
  state,
  onEndpointIdChange,
  onStateChange,
  onClearFilters,
  truncated,
  errorMessage,
}: {
  refetchIntervalMs: number;
  endpoints: EndpointListItem[] | undefined;
  endpointId: string | null;
  state: AttemptState | null;
  onEndpointIdChange: (value: string | null) => void;
  onStateChange: (value: AttemptState | null) => void;
  onClearFilters: () => void;
  truncated: boolean;
  errorMessage?: string | undefined;
}) {
  return (
    <>
      <div className="mt-4 flex items-center justify-between border-edge border-t pt-6">
        <h2 className="font-medium text-body text-foreground">Delivery attempts</h2>
        <p className="text-caption text-subtle-foreground">
          Refreshes every {Math.round(refetchIntervalMs / 1000)}s
        </p>
      </div>

      <div className="flex flex-wrap items-end gap-4">
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="attempts-endpoint">Endpoint</Label>
          <Select
            value={endpointId ?? "__all"}
            onValueChange={(value) => onEndpointIdChange(value === "__all" ? null : value)}
          >
            <SelectTrigger id="attempts-endpoint" className="w-[220px] sm:w-[280px]">
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
        </div>
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="attempts-state">State</Label>
          <Select
            value={state ?? "__all"}
            onValueChange={(value) =>
              onStateChange(value === "__all" ? null : (value as AttemptState))
            }
          >
            <SelectTrigger id="attempts-state" className="w-[160px] sm:w-[200px]">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="__all">All states</SelectItem>
              {ATTEMPT_STATES.map((s) => (
                <SelectItem key={s} value={s}>
                  {s}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
        {(endpointId !== null || state !== null) && (
          <Button type="button" variant="ghost" size="sm" onClick={onClearFilters}>
            Clear filters
          </Button>
        )}
      </div>

      {truncated && (
        <p className="text-caption text-subtle-foreground">
          Showing the most recent 1000 attempts — filtering happens over that window.
        </p>
      )}

      {errorMessage != null && (
        <div className="rounded-sm border border-state-danger-border bg-state-danger-bg px-3 py-2 text-caption text-state-danger-fg">
          Could not read attempts: {errorMessage}
        </div>
      )}
    </>
  );
}
