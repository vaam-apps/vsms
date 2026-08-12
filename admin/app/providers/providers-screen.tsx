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
// `packages/gateway/src/providers.ts` now resolves its Bearer token via
// `resolveUpstreamAccessToken()` (`./request-credential.ts`), which
// forwards the signed-in operator's own session token for an ordinary
// admin-console request — see that module's own doc for the mechanism.
// Save genuinely succeeds for a signed-in `owner`/`admin`/`operator`
// carrying the `provider:update` permission (seeded by `0002_bootstrap`
// for all three roles), and genuinely still 403s for a role that lacks it
// (e.g. `auditor`) — Layer 2 real, not defense in depth, per #211's own
// PR description.
//
// # Why no live poll
//
// A provider's config changes at the pace of an operator's own edits, not
// a worker's — `workers-screen.tsx`'s 5s `refetchInterval` fits a lease
// that can flip in ~5s; this table doesn't need that cadence. Plain
// `useQuery`, refetched on demand (closing the edit dialog) rather than on
// a timer.

import type { inferRouterOutputs } from "@trpc/server";
import type { AppRouter } from "@vsms/api";
import { trpc } from "@vsms/hooks";
import {
  Button,
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  IdDisplay,
  InlineEmptyState,
  Input,
  Label,
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
  ThemeToggle,
  TimestampDisplay,
  toast,
} from "@vsms/ui";
import { useEffect, useState } from "react";

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

interface EditFormState {
  displayName: string;
  state: ProviderState;
  maxTps: string;
  maxDailySubmissions: string;
  costPerSegmentXaf: string;
}

export function ProvidersScreen() {
  const listQuery = trpc.providers.list.useQuery();
  const utils = trpc.useUtils();

  const [selectedId, setSelectedId] = useState<string | null>(null);
  const detailQuery = trpc.providers.get.useQuery(
    { id: selectedId ?? "" },
    { enabled: selectedId !== null },
  );

  const [form, setForm] = useState<EditFormState | null>(null);

  useEffect(() => {
    if (detailQuery.data?.data !== undefined) {
      const d = detailQuery.data.data;
      setForm({
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
      setSelectedId(null);
      void utils.providers.list.invalidate();
    },
  });

  function closeDialog() {
    setSelectedId(null);
    setForm(null);
    updateMutation.reset();
  }

  function save() {
    if (selectedId === null || form === null || detailQuery.data?.etag === undefined) return;
    updateMutation.mutate({
      id: selectedId,
      etag: detailQuery.data.etag,
      displayName: form.displayName,
      state: form.state,
      maxTps: Number(form.maxTps),
      maxDailySubmissions: Number(form.maxDailySubmissions),
      costPerSegmentXaf: form.costPerSegmentXaf,
    });
  }

  return (
    <main className="mx-auto flex max-w-[1200px] flex-col gap-6 px-6 py-10">
      <header className="flex items-start justify-between gap-4 border-edge border-b pb-6">
        <div>
          <p className="font-mono text-micro text-subtle-foreground tracking-[0.03em]">
            vsms admin console
          </p>
          <h1 className="mt-1 font-medium text-foreground text-title">Providers</h1>
          <p className="mt-1 max-w-xl text-body text-muted-foreground">
            Every configured SMS provider — capacity, cost, and current state.
          </p>
        </div>
        <div className="flex shrink-0 items-center gap-3">
          <a
            href="/dashboard"
            className="text-caption text-muted-foreground underline decoration-edge-strong underline-offset-2 hover:decoration-foreground"
          >
            Dashboard
          </a>
          <a
            href="/routes"
            className="text-caption text-muted-foreground underline decoration-edge-strong underline-offset-2 hover:decoration-foreground"
          >
            Routes
          </a>
          <a
            href="/simulator"
            className="text-caption text-muted-foreground underline decoration-edge-strong underline-offset-2 hover:decoration-foreground"
          >
            Route simulator
          </a>
          <a
            href="/"
            className="text-caption text-muted-foreground underline decoration-edge-strong underline-offset-2 hover:decoration-foreground"
          >
            Composer
          </a>
          <ThemeToggle />
        </div>
      </header>

      <div className="rounded-sm border border-edge bg-surface-2 px-3 py-2 text-caption text-muted-foreground">
        Reads and writes both act as you, not as a shared service account — Save requires your own
        role to carry <span className="font-mono text-foreground">provider:update</span> (owner,
        admin, and operator all do by default). A role without it, or a stale edit someone else
        already saved, will surface as a real error here rather than silently failing.
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
            <TableHead>Key</TableHead>
            <TableHead>Display name</TableHead>
            <TableHead>Kind</TableHead>
            <TableHead>Healthy</TableHead>
            <TableHead align="end">Max TPS</TableHead>
            <TableHead align="end">Cost/segment (XAF)</TableHead>
            <TableHead align="end">Updated</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {listQuery.isLoading &&
            Array.from({ length: 4 }).map((_, i) => (
              // biome-ignore lint/suspicious/noArrayIndexKey: static skeleton rows, never reordered or diffed
              <TableRow key={i}>
                <TableCell colSpan={8}>
                  <Skeleton className="h-4 w-full" />
                </TableCell>
              </TableRow>
            ))}

          {!listQuery.isLoading && (listQuery.data?.length ?? 0) === 0 && (
            <tr>
              <td colSpan={8}>
                <InlineEmptyState message="No providers configured yet." />
              </td>
            </tr>
          )}

          {listQuery.data?.map((provider: ProviderListItem) => (
            <TableRow
              key={provider.id}
              className="cursor-pointer"
              onClick={() => setSelectedId(provider.id)}
            >
              <TableCell>
                <StatePill state={provider.state} />
              </TableCell>
              <TableCell mono>{provider.key}</TableCell>
              <TableCell>{provider.displayName}</TableCell>
              <TableCell mono>{provider.kind}</TableCell>
              <TableCell>
                {provider.healthy ? (
                  <span className="text-state-success-fg">yes</span>
                ) : (
                  <span className="text-muted-foreground">no probe yet</span>
                )}
              </TableCell>
              <TableCell align="end" mono>
                {provider.maxTps}
              </TableCell>
              <TableCell align="end" mono>
                {provider.costPerSegmentXaf}
              </TableCell>
              <TableCell align="end">
                <TimestampDisplay value={provider.updatedAt} />
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>

      <Dialog open={selectedId !== null} onOpenChange={(open) => !open && closeDialog()}>
        <DialogContent className="max-w-[520px]">
          <DialogHeader>
            <DialogTitle>
              {detailQuery.data?.data !== undefined
                ? detailQuery.data.data.displayName
                : "Provider"}
            </DialogTitle>
            <DialogDescription>
              {selectedId !== null && <IdDisplay value={selectedId} variant="full" />}
            </DialogDescription>
          </DialogHeader>

          {detailQuery.isLoading && <Skeleton className="h-32 w-full" />}

          {detailQuery.data?.data !== undefined && form !== null && (
            <div className="flex flex-col gap-4">
              <div className="grid grid-cols-2 gap-3 rounded-sm border border-edge bg-surface-2 p-3 text-caption text-muted-foreground">
                <div>
                  Key:{" "}
                  <span className="font-mono text-foreground">{detailQuery.data.data.key}</span>
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
                  Key/kind/config/credential ref are infrastructure wiring, set once at provisioning
                  — not editable from this form.
                </div>
              </div>

              <div className="flex flex-col gap-1.5">
                <Label htmlFor="provider-display-name">Display name</Label>
                <Input
                  id="provider-display-name"
                  value={form.displayName}
                  onChange={(e) => setForm({ ...form, displayName: e.target.value })}
                />
              </div>

              <div className="flex flex-col gap-1.5">
                <Label htmlFor="provider-state">State</Label>
                <Select
                  value={form.state}
                  onValueChange={(value) => setForm({ ...form, state: value as ProviderState })}
                >
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
              </div>

              <div className="grid grid-cols-2 gap-3">
                <div className="flex flex-col gap-1.5">
                  <Label htmlFor="provider-max-tps">Max TPS</Label>
                  <Input
                    id="provider-max-tps"
                    type="number"
                    min="0"
                    step="0.1"
                    value={form.maxTps}
                    onChange={(e) => setForm({ ...form, maxTps: e.target.value })}
                  />
                </div>
                <div className="flex flex-col gap-1.5">
                  <Label htmlFor="provider-max-daily">Max daily submissions</Label>
                  <Input
                    id="provider-max-daily"
                    type="number"
                    min="0"
                    step="1"
                    value={form.maxDailySubmissions}
                    onChange={(e) => setForm({ ...form, maxDailySubmissions: e.target.value })}
                  />
                </div>
              </div>

              <div className="flex flex-col gap-1.5">
                <Label htmlFor="provider-cost">Cost per segment (XAF)</Label>
                <Input
                  id="provider-cost"
                  value={form.costPerSegmentXaf}
                  onChange={(e) => setForm({ ...form, costPerSegmentXaf: e.target.value })}
                />
              </div>

              {updateMutation.isError && (
                <div className="rounded-sm border border-state-danger-border bg-state-danger-bg px-3 py-2 text-caption text-state-danger-fg">
                  Save failed: {updateMutation.error.message}
                </div>
              )}
            </div>
          )}

          <DialogFooter>
            <Button type="button" variant="ghost" onClick={closeDialog}>
              Close
            </Button>
            <Button
              type="button"
              disabled={updateMutation.isPending || detailQuery.data?.etag === undefined}
              onClick={save}
            >
              {updateMutation.isPending ? "Saving…" : "Save"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </main>
  );
}
