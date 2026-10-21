// Dumb component (R6, route-local): every configured provider as a card.
//
// A deployment has one to four providers, so the screen used to be a table
// whose capacity and cost columns disappeared below `sm`/`lg` — on a phone,
// a state pill and a name. A card per provider keeps state, health, TPS and
// cost visible at every width. A card is a button and opens the same quick
// detail the row did; the edit drawer is one more tap, unchanged.

import { Skeleton, TimestampDisplay } from "@vaam-apps/ui";
import { ChevronRight } from "lucide-react";
import type { ProviderState } from "../provider-types";
import { StatePill } from "./state-pill";

export interface ProviderRow {
  id: string;
  state: ProviderState;
  displayName: string;
  key: string;
  kind: string;
  healthy: boolean;
  maxTps: number;
  costPerSegmentXaf: string;
  updatedAt: string;
}

export interface ProviderCardGridProps {
  rows: ProviderRow[];
  isLoading: boolean;
  onOpen: (id: string) => void;
}

function Figure({ label, value }: { label: string; value: string }) {
  return (
    <span className="flex flex-col gap-0.5">
      <span className="text-label-small text-muted-foreground">{label}</span>
      <span className="font-mono text-body-medium text-foreground">{value}</span>
    </span>
  );
}

export function ProviderCardGrid({ rows, isLoading, onOpen }: ProviderCardGridProps) {
  if (isLoading) {
    return (
      <div className="grid gap-3 md:grid-cols-2 xl:grid-cols-3">
        {["a", "b", "c"].map((key) => (
          <Skeleton key={key} className="h-44 rounded-[20px]" />
        ))}
      </div>
    );
  }
  if (rows.length === 0) {
    return (
      <div className="rounded-[20px] border border-edge-subtle border-dashed p-8 text-center text-body-medium text-muted-foreground">
        No providers configured yet. Seed one with{" "}
        <span className="font-mono">sms-gateway seed-dispatch</span>.
      </div>
    );
  }
  return (
    <ul className="grid gap-3 md:grid-cols-2 xl:grid-cols-3">
      {rows.map((provider) => (
        <li key={provider.id}>
          <button
            type="button"
            onClick={() => onOpen(provider.id)}
            aria-label={`${provider.displayName} details`}
            className="flex h-full w-full flex-col gap-4 rounded-[20px] border border-edge-subtle bg-base-100 p-5 text-left transition-colors hover:bg-surface-1 focus-visible:outline-2 focus-visible:outline-primary"
          >
            <span className="flex items-start justify-between gap-3">
              <span className="flex min-w-0 flex-col gap-1">
                <span className="truncate text-title-medium text-foreground">
                  {provider.displayName}
                </span>
                <span className="truncate font-mono text-body-small text-muted-foreground">
                  {provider.key} · {provider.kind}
                </span>
              </span>
              <ChevronRight
                size={18}
                aria-hidden="true"
                className="mt-1 shrink-0 text-subtle-foreground"
              />
            </span>
            <span className="flex flex-wrap items-center gap-2">
              <StatePill state={provider.state} />
              <span
                className={
                  provider.healthy
                    ? "text-body-small text-state-success-fg"
                    : "text-body-small text-muted-foreground"
                }
              >
                {provider.healthy ? "Healthy" : "Not probed yet"}
              </span>
            </span>
            <span className="mt-auto grid grid-cols-2 gap-3">
              <Figure label="Max TPS" value={String(provider.maxTps)} />
              <Figure label="Cost per segment" value={`${provider.costPerSegmentXaf} XAF`} />
            </span>
            <span className="text-body-small text-muted-foreground">
              Updated <TimestampDisplay value={provider.updatedAt} />
            </span>
          </button>
        </li>
      ))}
    </ul>
  );
}
