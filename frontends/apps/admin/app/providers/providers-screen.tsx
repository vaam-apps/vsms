"use client";

// The Providers screen (#54): list and detail, plus editing — real, tested
// code, and (as of #211) real writes against a real gateway for a
// signed-in `owner`/`admin`/`operator`. This screen is #211's own named
// proof case: it was the concrete example that exposed the console
// forwarding its machine credential regardless of who was signed in.
//
// # Reads and writes both work today, for a real signed-in human
//
// `Provider.read`'s `@@allow` admits `auth().kind == "app"` (#54,
// `schema.cstack`) alongside the human roles. `Provider.update` stays
// `hasRole('owner') || hasRole('admin') || hasRole('operator')` only — no
// `auth().kind == "app"` clause at all, so it was never reachable by this
// console's own machine credential and needed a real human principal.
// `frontends/packages/gateway/src/providers.ts` resolves its Bearer token via
// `resolveUpstreamAccessToken()` (`./request-credential.ts`), which
// forwards the signed-in operator's own session token for an ordinary
// admin-console request — see that module's own doc for the mechanism.
// Save genuinely succeeds for a signed-in `owner`/`admin`/`operator`
// carrying the `provider:update` permission, and genuinely still 403s for
// a role that lacks it (e.g. `auditor`) — Layer 2 real, not defense in
// depth. The failure surfaces verbatim in the edit drawer's own error
// banner below (`updateMutation.error.message`) — never swallowed, never
// silently retried.
//
// # Quick vs. more detail (console-redesign.md §3/D14)
//
// A row click opens `QuickDetailDrawer` — a narrow, undimmed peek at the
// fields already on the list row plus the ones one fetch away (credential
// wiring, healthy/last-probed). "Edit" upgrades to `MoreDetailDrawer`,
// which owns a shallow `?panel=<id>` route (survives refresh, linkable)
// and holds the real edit form. Quick detail owns no route — closing it
// and reopening the same row is one click, per D14.
//
// # Why no live poll
//
// A provider's config changes at the pace of an operator's own edits, not
// a worker's — `workers-screen.tsx`'s 5s `refetchInterval` fits a lease
// that can flip in ~5s; this table doesn't need that cadence. Plain
// `useQuery`, refetched on demand (closing the edit drawer) rather than on
// a timer.

import { zodResolver } from "@hookform/resolvers/zod";
import type { inferRouterOutputs } from "@trpc/server";
import type { AppRouter } from "@vsms/api";
import { trpc } from "@vsms/hooks";
import {
  Button,
  IdDisplay,
  InlineEmptyState,
  Input,
  Label,
  MoreDetailDrawer,
  QuickDetailDrawer,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  Skeleton,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
  TimestampDisplay,
  toast,
} from "@vsms/ui";
import { parseAsString, useQueryState } from "nuqs";
import { useEffect, useState } from "react";
import { Controller, useForm } from "react-hook-form";
import { z } from "zod";

type RouterOutputs = inferRouterOutputs<AppRouter>;
type ProviderListItem = RouterOutputs["providers"]["list"][number];

const PROVIDER_STATES = ["active", "degraded", "disabled", "draining"] as const;
type ProviderState = (typeof PROVIDER_STATES)[number];

const STATE_CLASSES: Record<ProviderState, string> = {
  active: "border-state-success-border bg-state-success-bg text-state-success-fg",
  degraded: "border-state-uncertain-border bg-state-uncertain-bg text-state-uncertain-fg",
  disabled: "border-state-danger-border bg-state-danger-bg text-state-danger-fg",
  draining: "border-state-uncertain-border bg-state-uncertain-bg text-state-uncertain-fg",
};

function StatePill({ state }: { state: ProviderState }) {
  return (
    <span className={`rounded-sm border px-1.5 py-0.5 text-caption ${STATE_CLASSES[state]}`}>
      {state}
    </span>
  );
}

// Mirrors `UpdateProviderFields` (`packages/gateway/src/providers.ts`) — the
// operationally-relevant subset this screen lets a human edit.
const editSchema = z.object({
  displayName: z.string().trim().min(1, "Display name is required"),
  state: z.enum(PROVIDER_STATES),
  maxTps: z
    .string()
    .trim()
    .refine((v) => v !== "" && Number.isFinite(Number(v)) && Number(v) >= 0, "Enter a number ≥ 0"),
  maxDailySubmissions: z
    .string()
    .trim()
    .refine(
      (v) => v !== "" && Number.isInteger(Number(v)) && Number(v) >= 0,
      "Enter a whole number ≥ 0",
    ),
  costPerSegmentXaf: z.string().trim().min(1, "Cost per segment is required"),
});
type EditFormValues = z.infer<typeof editSchema>;

export function ProvidersScreen() {
  const listQuery = trpc.providers.list.useQuery();
  const utils = trpc.useUtils();

  // Quick detail: local state, no route ownership (D14) — losing it on
  // refresh is fine, reopening is one click on the same row.
  const [quickId, setQuickId] = useState<string | null>(null);
  const quickDetail = listQuery.data?.find((p) => p.id === quickId);

  // More detail: owns `?panel=<id>` so it survives refresh and is
  // linkable — the caller-owned routing D14 asks for; `MoreDetailDrawer`
  // itself has no opinion on it.
  const [panelId, setPanelId] = useQueryState("panel", parseAsString);
  const detailQuery = trpc.providers.get.useQuery(
    { id: panelId ?? "" },
    { enabled: panelId !== null },
  );

  const form = useForm<EditFormValues>({ resolver: zodResolver(editSchema) });

  // biome-ignore lint/correctness/useExhaustiveDependencies: `form` is stable across renders — only re-seed when the fetched record changes.
  useEffect(() => {
    const d = detailQuery.data?.data;
    if (d !== undefined) {
      form.reset({
        displayName: d.displayName,
        state: d.state,
        maxTps: String(d.maxTps),
        maxDailySubmissions: String(d.maxDailySubmissions),
        costPerSegmentXaf: d.costPerSegmentXaf,
      });
    }
  }, [detailQuery.data]);

  const updateMutation = trpc.providers.update.useMutation({
    onSuccess: () => {
      toast({ title: "Provider saved", variant: "success" });
      void setPanelId(null);
      void utils.providers.list.invalidate();
    },
  });

  function closeMore() {
    void setPanelId(null);
    updateMutation.reset();
  }

  function onSubmit(values: EditFormValues) {
    if (panelId === null || detailQuery.data?.etag === undefined) return;
    updateMutation.mutate({
      id: panelId,
      etag: detailQuery.data.etag,
      displayName: values.displayName,
      state: values.state,
      maxTps: Number(values.maxTps),
      maxDailySubmissions: Number(values.maxDailySubmissions),
      costPerSegmentXaf: values.costPerSegmentXaf,
    });
  }

  return (
    <main className="mx-auto flex max-w-[1400px] flex-col gap-6 px-4 py-6 sm:px-6 sm:py-10">
      <header className="flex flex-col gap-1 border-edge border-b pb-6">
        <p className="font-mono text-micro text-subtle-foreground tracking-[0.03em]">
          vsms admin console
        </p>
        <h1 className="font-medium text-foreground text-title">Providers</h1>
        <p className="max-w-xl text-body text-muted-foreground">
          Every configured SMS provider — capacity, cost, and current state.
        </p>
      </header>

      <div className="rounded-sm border border-edge bg-surface-2 px-3 py-2 text-caption text-muted-foreground">
        Reads and writes both act as you, not as a shared service account — Save requires your own
        role to carry <span className="font-mono text-foreground">provider:update</span> (owner,
        admin, and operator all do by default). A role without it, or a stale edit someone else
        already saved, surfaces as a real error here rather than silently failing.
      </div>

      {listQuery.isError && (
        <div className="rounded-sm border border-state-danger-border bg-state-danger-bg px-3 py-2 text-caption text-state-danger-fg">
          Could not read providers: {listQuery.error.message}
        </div>
      )}

      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>State</TableHead>
            <TableHead>Provider</TableHead>
            <TableHead className="hidden md:table-cell">Kind</TableHead>
            <TableHead className="hidden sm:table-cell">Healthy</TableHead>
            <TableHead align="end" className="hidden sm:table-cell">
              Max TPS
            </TableHead>
            <TableHead align="end" className="hidden lg:table-cell">
              Cost/segment (XAF)
            </TableHead>
            <TableHead align="end" className="hidden md:table-cell">
              Updated
            </TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {listQuery.isLoading &&
            Array.from({ length: 4 }).map((_, i) => (
              // biome-ignore lint/suspicious/noArrayIndexKey: static skeleton rows, never reordered or diffed
              <TableRow key={i}>
                <TableCell colSpan={7}>
                  <Skeleton className="h-4 w-full" />
                </TableCell>
              </TableRow>
            ))}

          {!listQuery.isLoading && (listQuery.data?.length ?? 0) === 0 && (
            <tr>
              <td colSpan={7}>
                <InlineEmptyState message="No providers configured yet." />
              </td>
            </tr>
          )}

          {listQuery.data?.map((provider: ProviderListItem) => (
            <TableRow
              key={provider.id}
              className="cursor-pointer"
              onClick={() => setQuickId(provider.id)}
            >
              <TableCell>
                <StatePill state={provider.state} />
              </TableCell>
              <TableCell>
                <div className="flex flex-col">
                  <span>{provider.displayName}</span>
                  <span className="font-mono text-caption text-subtle-foreground">
                    {provider.key}
                  </span>
                </div>
              </TableCell>
              <TableCell mono className="hidden md:table-cell">
                {provider.kind}
              </TableCell>
              <TableCell className="hidden sm:table-cell">
                {provider.healthy ? (
                  <span className="text-state-success-fg">yes</span>
                ) : (
                  <span className="text-muted-foreground">no probe yet</span>
                )}
              </TableCell>
              <TableCell align="end" mono className="hidden sm:table-cell">
                {provider.maxTps}
              </TableCell>
              <TableCell align="end" mono className="hidden lg:table-cell">
                {provider.costPerSegmentXaf}
              </TableCell>
              <TableCell align="end" className="hidden md:table-cell">
                <TimestampDisplay value={provider.updatedAt} />
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>

      {/* Quick detail — a peek, no route, closes back to exactly where the
          list was. */}
      <QuickDetailDrawer
        open={quickId !== null}
        onOpenChange={(open) => !open && setQuickId(null)}
        title={quickDetail?.displayName ?? "Provider"}
        description={
          quickDetail !== undefined && <IdDisplay value={quickDetail.id} variant="full" />
        }
        footer={
          <>
            <Button type="button" variant="ghost" size="sm" onClick={() => setQuickId(null)}>
              Close
            </Button>
            <Button
              type="button"
              size="sm"
              onClick={() => {
                if (quickDetail === undefined) return;
                void setPanelId(quickDetail.id);
                setQuickId(null);
              }}
            >
              Edit
            </Button>
          </>
        }
      >
        {quickDetail !== undefined && (
          <dl className="flex flex-col gap-3 text-body">
            <div className="flex items-center justify-between gap-3">
              <dt className="text-muted-foreground">State</dt>
              <dd>
                <StatePill state={quickDetail.state} />
              </dd>
            </div>
            <div className="flex items-center justify-between gap-3">
              <dt className="text-muted-foreground">Key</dt>
              <dd className="font-mono text-caption">{quickDetail.key}</dd>
            </div>
            <div className="flex items-center justify-between gap-3">
              <dt className="text-muted-foreground">Kind</dt>
              <dd className="font-mono text-caption">{quickDetail.kind}</dd>
            </div>
            <div className="flex items-center justify-between gap-3">
              <dt className="text-muted-foreground">Healthy</dt>
              <dd>
                {quickDetail.healthy ? (
                  <span className="text-state-success-fg">yes</span>
                ) : (
                  <span className="text-muted-foreground">no probe yet</span>
                )}
              </dd>
            </div>
            <div className="flex items-center justify-between gap-3">
              <dt className="text-muted-foreground">Max TPS</dt>
              <dd className="font-mono">{quickDetail.maxTps}</dd>
            </div>
            <div className="flex items-center justify-between gap-3">
              <dt className="text-muted-foreground">Cost/segment (XAF)</dt>
              <dd className="font-mono">{quickDetail.costPerSegmentXaf}</dd>
            </div>
            <div className="flex items-center justify-between gap-3">
              <dt className="text-muted-foreground">Updated</dt>
              <dd>
                <TimestampDisplay value={quickDetail.updatedAt} />
              </dd>
            </div>
          </dl>
        )}
      </QuickDetailDrawer>

      {/* More detail — the full record and its edit form, owns
          `?panel=<id>` so it survives refresh (D14). */}
      <MoreDetailDrawer
        open={panelId !== null}
        onOpenChange={(open) => !open && closeMore()}
        title={detailQuery.data?.data?.displayName ?? "Provider"}
        description={panelId !== null && <IdDisplay value={panelId} variant="full" />}
        footer={
          <>
            <Button type="button" variant="ghost" onClick={closeMore}>
              Close
            </Button>
            <Button
              type="submit"
              form="provider-edit-form"
              disabled={updateMutation.isPending || detailQuery.data?.etag === undefined}
            >
              {updateMutation.isPending ? "Saving…" : "Save"}
            </Button>
          </>
        }
      >
        {detailQuery.isLoading && <Skeleton className="h-32 w-full" />}

        {detailQuery.data?.data !== undefined && (
          <form
            id="provider-edit-form"
            onSubmit={form.handleSubmit(onSubmit)}
            className="flex flex-col gap-4"
          >
            <div className="grid grid-cols-2 gap-3 rounded-sm border border-edge bg-surface-2 p-3 text-caption text-muted-foreground">
              <div>
                Key: <span className="font-mono text-foreground">{detailQuery.data.data.key}</span>
              </div>
              <div>
                Kind:{" "}
                <span className="font-mono text-foreground">{detailQuery.data.data.kind}</span>
              </div>
              <div className="col-span-2">
                Credential ref:{" "}
                <span className="font-mono text-foreground">
                  {detailQuery.data.data.credentialRef}
                </span>
              </div>
              <div className="col-span-2 text-subtle-foreground">
                Key/kind/config/credential ref are infrastructure wiring, set once at provisioning —
                not editable from this form.
              </div>
            </div>

            <div className="flex flex-col gap-1.5">
              <Label htmlFor="provider-display-name">Display name</Label>
              <Input
                id="provider-display-name"
                aria-invalid={form.formState.errors.displayName != null}
                {...form.register("displayName")}
              />
              {form.formState.errors.displayName != null && (
                <p className="text-caption text-state-danger-fg">
                  {form.formState.errors.displayName.message}
                </p>
              )}
            </div>

            <div className="flex flex-col gap-1.5">
              <Label htmlFor="provider-state">State</Label>
              <Controller
                control={form.control}
                name="state"
                render={({ field }) => (
                  <Select value={field.value} onValueChange={field.onChange}>
                    <SelectTrigger id="provider-state">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {PROVIDER_STATES.map((state) => (
                        <SelectItem key={state} value={state}>
                          {state}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                )}
              />
            </div>

            <div className="grid grid-cols-2 gap-3">
              <div className="flex flex-col gap-1.5">
                <Label htmlFor="provider-max-tps">Max TPS</Label>
                <Input
                  id="provider-max-tps"
                  inputMode="decimal"
                  aria-invalid={form.formState.errors.maxTps != null}
                  {...form.register("maxTps")}
                />
                {form.formState.errors.maxTps != null && (
                  <p className="text-caption text-state-danger-fg">
                    {form.formState.errors.maxTps.message}
                  </p>
                )}
              </div>
              <div className="flex flex-col gap-1.5">
                <Label htmlFor="provider-max-daily">Max daily submissions</Label>
                <Input
                  id="provider-max-daily"
                  inputMode="numeric"
                  aria-invalid={form.formState.errors.maxDailySubmissions != null}
                  {...form.register("maxDailySubmissions")}
                />
                {form.formState.errors.maxDailySubmissions != null && (
                  <p className="text-caption text-state-danger-fg">
                    {form.formState.errors.maxDailySubmissions.message}
                  </p>
                )}
              </div>
            </div>

            <div className="flex flex-col gap-1.5">
              <Label htmlFor="provider-cost">Cost per segment (XAF)</Label>
              <Input
                id="provider-cost"
                aria-invalid={form.formState.errors.costPerSegmentXaf != null}
                {...form.register("costPerSegmentXaf")}
              />
              {form.formState.errors.costPerSegmentXaf != null && (
                <p className="text-caption text-state-danger-fg">
                  {form.formState.errors.costPerSegmentXaf.message}
                </p>
              )}
            </div>

            {updateMutation.isError && (
              <div className="rounded-sm border border-state-danger-border bg-state-danger-bg px-3 py-2 text-caption text-state-danger-fg">
                Save failed: {updateMutation.error.message}
              </div>
            )}
          </form>
        )}
      </MoreDetailDrawer>
    </main>
  );
}
