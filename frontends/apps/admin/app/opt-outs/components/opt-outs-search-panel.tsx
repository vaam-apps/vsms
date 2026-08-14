// Dumb component (R6): the search box and its result panel, moved verbatim
// out of `opt-outs-screen.tsx`. Fetching moved out too — this file used to
// call `trpc.optOuts.search.useQuery` itself, a dumb component reaching
// for its own data regardless of where its classes lived. The smart screen
// now owns the query and hands the result down as `result`.

import type { inferRouterOutputs } from "@trpc/server";
import type { AppRouter } from "@vsms/api";
import { Button, Input, Label, Skeleton, TimestampDisplay } from "@vsms/ui";

type RouterOutputs = inferRouterOutputs<AppRouter>;
export type OptOutSearchResult = RouterOutputs["optOuts"]["search"];

export interface OptOutsSearchPanelProps {
  msisdn: string;
  onMsisdnChange: (value: string) => void;
  canSearch: boolean;
  onSearch: () => void;
  searchedFor: string | null;
  isLoading: boolean;
  isError: boolean;
  errorMessage?: string | undefined;
  result?: OptOutSearchResult | undefined;
}

export function OptOutsSearchPanel({
  msisdn,
  onMsisdnChange,
  canSearch,
  onSearch,
  searchedFor,
  isLoading,
  isError,
  errorMessage,
  result,
}: OptOutsSearchPanelProps) {
  return (
    <div className="flex flex-col gap-3 rounded-sm border border-edge bg-surface-2 p-4">
      <div className="flex flex-col items-stretch gap-2 sm:flex-row sm:items-end">
        <div className="flex flex-1 flex-col gap-1.5">
          <Label htmlFor="opt-out-search">Search by MSISDN</Label>
          <Input
            id="opt-out-search"
            placeholder="+237677123456"
            value={msisdn}
            onChange={(e) => onMsisdnChange(e.target.value)}
          />
        </div>
        <Button type="button" disabled={!canSearch} onClick={onSearch}>
          Search
        </Button>
      </div>

      {searchedFor !== null && (
        <div className="rounded-sm border border-edge bg-surface-1 p-3">
          {isLoading && <Skeleton className="h-6 w-full" />}
          {isError && <p className="text-caption text-state-danger-fg">{errorMessage}</p>}
          {result !== undefined && result.optOut !== undefined && (
            <div className="flex flex-col gap-1 text-caption">
              <p className="text-state-danger-fg">
                Opted out — source <span className="font-mono">{result.optOut.source}</span>, scope{" "}
                <span className="font-mono">{result.optOut.scope}</span>
              </p>
              <p className="text-muted-foreground">
                <TimestampDisplay value={result.optOut.optedOutAt} />
                {result.optOut.reason !== undefined && <> — {result.optOut.reason}</>}
              </p>
            </div>
          )}
          {result !== undefined && result.optOut === undefined && (
            <p className="text-caption text-state-success-fg">No opt-out found for that number.</p>
          )}
          <p className="mt-2 text-micro text-subtle-foreground">
            This can never distinguish &quot;never opted out&quot; from &quot;opted out before the
            hash pepper was last rotated&quot; — a rotation orphans older hashes silently and
            permanently. Treat a &quot;not found&quot; result as inconclusive for a number with any
            history predating a known rotation, not as proof.
          </p>
        </div>
      )}
    </div>
  );
}
