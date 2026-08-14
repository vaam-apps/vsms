"use client";

import { useState } from "react";
import { cn } from "../../lib/cn";
// D18: `Tabs` was rebuilt as `ValueTabs` (Headless UI `TabGroup` behind a
// value-based adapter) — aliased on import so nothing below this line
// changes; see tabs.tsx's own module doc for the full mechanism.
import {
  ValueTabs as Tabs,
  ValueTabsContent as TabsContent,
  ValueTabsList as TabsList,
  ValueTabsTrigger as TabsTrigger,
} from "../primitives/tabs";

export interface PayloadExchange {
  direction: "request" | "response" | "callback";
  method?: string;
  url?: string;
  status?: number;
  durationMs?: number;
  headers?: Record<string, string>;
  body?: string;
  error?: string;
}

export interface PayloadInspectorProps {
  exchanges: PayloadExchange[];
  defaultOpen?: number;
  /** Bodies over this size collapse behind an explicit "load full payload" action. */
  maxInlineBytes?: number;
}

function StatusChip({ status }: { status?: number | undefined }) {
  if (status == null) return null;
  const ok = status < 400;
  return (
    <span
      className={cn(
        "rounded-sm border px-1.5 py-0.5 font-mono text-caption",
        ok
          ? "border-edge text-muted-foreground"
          : "border-state-danger-border text-state-danger-fg",
      )}
    >
      {status}
    </span>
  );
}

function ExchangeBody({
  exchange,
  maxInlineBytes,
}: {
  exchange: PayloadExchange;
  maxInlineBytes: number;
}) {
  const [forceLoad, setForceLoad] = useState(false);
  const body = exchange.body ?? "";
  const oversized = !forceLoad && body.length > maxInlineBytes;

  return (
    <Tabs defaultValue="body">
      <TabsList>
        <TabsTrigger value="body">Body</TabsTrigger>
        <TabsTrigger value="headers">Headers</TabsTrigger>
        {exchange.error != null && <TabsTrigger value="error">Error</TabsTrigger>}
      </TabsList>
      <TabsContent value="body">
        {body.length === 0 ? (
          <p className="py-3 text-caption text-subtle-foreground">No response body was recorded.</p>
        ) : oversized ? (
          <button
            type="button"
            onClick={() => setForceLoad(true)}
            className="py-3 text-caption text-foreground underline"
          >
            Load full payload ({(body.length / 1024).toFixed(1)} KB)
          </button>
        ) : (
          <pre className="max-h-96 overflow-auto rounded-sm bg-base-100 p-3 font-mono text-[12px] text-foreground">
            {body}
          </pre>
        )}
      </TabsContent>
      <TabsContent value="headers">
        {exchange.headers == null || Object.keys(exchange.headers).length === 0 ? (
          <p className="py-3 text-caption text-subtle-foreground">No headers recorded.</p>
        ) : (
          <dl className="grid grid-cols-[max-content_1fr] gap-x-3 gap-y-1 py-2 font-mono text-[12px]">
            {Object.entries(exchange.headers).map(([key, value]) => (
              <div key={key} className="contents">
                <dt className="text-subtle-foreground">{key}</dt>
                <dd className="break-all text-foreground">{value}</dd>
              </div>
            ))}
          </dl>
        )}
      </TabsContent>
      {exchange.error != null && (
        <TabsContent value="error">
          <pre className="max-h-96 overflow-auto rounded-sm border border-state-danger-border bg-state-danger-bg p-3 font-mono text-[12px] text-state-danger-fg">
            {exchange.error}
          </pre>
        </TabsContent>
      )}
    </Tabs>
  );
}

/**
 * Raw provider request/response and DLR bodies, verbatim (design doc §5.3).
 * No syntax-highlighting palette on purpose (§1.3): a rainbow JSON block
 * would be the loudest thing on a diagnostic screen. Uses native
 * `<details>` for the accordion behaviour — free keyboard/ARIA semantics,
 * no extra Radix dependency needed for a plain expand/collapse.
 */
export function PayloadInspector({
  exchanges,
  defaultOpen = 0,
  maxInlineBytes = 262144,
}: PayloadInspectorProps) {
  return (
    <div className="flex flex-col gap-2">
      {exchanges.map((exchange, i) => (
        <details
          // biome-ignore lint/suspicious/noArrayIndexKey: exchanges are a fixed, ordered, append-only log
          key={i}
          open={i === defaultOpen}
          className="rounded-sm border border-edge bg-surface-2"
        >
          <summary className="flex cursor-pointer list-none items-center gap-2 px-3 py-2 text-body">
            <span className="font-mono text-micro text-muted-foreground">{exchange.direction}</span>
            {exchange.method != null && (
              <span className="font-mono text-caption text-foreground">{exchange.method}</span>
            )}
            {exchange.url != null && (
              <span className="truncate font-mono text-caption text-subtle-foreground">
                {exchange.url}
              </span>
            )}
            <span className="ml-auto flex items-center gap-2">
              <StatusChip status={exchange.status} />
              {exchange.durationMs != null && (
                <span className="font-mono text-caption text-subtle-foreground">
                  {exchange.durationMs}ms
                </span>
              )}
            </span>
          </summary>
          <div className="border-edge border-t px-3 pb-3">
            <ExchangeBody exchange={exchange} maxInlineBytes={maxInlineBytes} />
          </div>
        </details>
      ))}
    </div>
  );
}
