// Dumb component (R6): the search box and its result panel. A `<form>`
// now, so Enter searches (it used to need a click), in an M3 search-bar
// shape. Originally moved verbatim out of `opt-outs-screen.tsx`. Fetching moved out too — this file used to
// call `trpc.optOuts.search.useQuery` itself, a dumb component reaching
// for its own data regardless of where its classes lived. The smart screen
// now owns the query and hands the result down as `result`.

import type { inferRouterOutputs } from "@trpc/server";
import { Button, FieldError, Input, Skeleton, TimestampDisplay } from "@vaam-apps/ui";
import type { AppRouter } from "@vsms/api";
import { Search } from "lucide-react";

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
    <section className="flex flex-col gap-3 rounded-[20px] bg-surface-container-low p-4 sm:p-5">
      <form
        aria-label="Opt-out search"
        className="flex flex-col gap-2 sm:flex-row"
        onSubmit={(e) => {
          e.preventDefault();
          if (canSearch) onSearch();
        }}
      >
        <div className="relative flex-1">
          <Search
            size={18}
            aria-hidden="true"
            className="pointer-events-none absolute top-1/2 left-4 z-10 -translate-y-1/2 text-muted-foreground"
          />
          <Input
            id="opt-out-search"
            aria-label="Search by phone number"
            placeholder="Is this number opted out? +237677123456"
            className="h-12 w-full rounded-full pl-11"
            inputMode="tel"
            value={msisdn}
            onChange={(e) => onMsisdnChange(e.target.value)}
          />
        </div>
        <Button type="submit" disabled={!canSearch} className="h-12 rounded-full px-6">
          Search
        </Button>
      </form>

      {searchedFor !== null && (
        <div aria-live="polite" className="rounded-[16px] bg-base-100 p-4">
          {isLoading && <Skeleton className="h-6 w-full" />}
          {isError && <FieldError>{errorMessage}</FieldError>}
          {result !== undefined && result.optOut !== undefined && (
            <div className="flex flex-col gap-1">
              <p className="text-body-large text-state-danger-fg">{searchedFor} is opted out</p>
              <p className="text-body-small text-muted-foreground">
                Since <TimestampDisplay value={result.optOut.optedOutAt} /> · source{" "}
                <span className="font-mono">{result.optOut.source}</span> · scope{" "}
                <span className="font-mono">{result.optOut.scope}</span>
                {result.optOut.reason !== undefined && <> · {result.optOut.reason}</>}
              </p>
            </div>
          )}
          {result !== undefined && result.optOut === undefined && (
            <p className="text-body-large text-foreground">No opt-out found for {searchedFor}.</p>
          )}
          <p className="mt-2 text-body-small text-muted-foreground">
            A number that opted out before the hash pepper was last rotated also reads as not found,
            so treat &quot;not found&quot; as inconclusive for a number with older history.
          </p>
        </div>
      )}
    </section>
  );
}
