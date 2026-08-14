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
// and reopening the same row is one click, per D14. `quickId` is therefore
// kept as local `useState` rather than URL state: it's presentational,
// single-value, and losing it on refresh is the documented, intended
// behaviour (D14), not an oversight R6's "avoid useState" guidance argues
// against.
//
// # Why no live poll
//
// A provider's config changes at the pace of an operator's own edits, not
// a worker's — `workers-screen.tsx`'s 5s `refetchInterval` fits a lease
// that can flip in ~5s; this table doesn't need that cadence. Plain
// `useQuery`, refetched on demand (closing the edit drawer) rather than on
// a timer.
//
// # R6
//
// This file holds data fetching, mutations, URL state and handlers only —
// every class and every piece of markup lives in `./components/*` and
// `./provider-types.ts`/`./edit-schema.ts`.

import { zodResolver } from "@hookform/resolvers/zod";
import type { inferRouterOutputs } from "@trpc/server";
import type { AppRouter } from "@vsms/api";
import { trpc } from "@vsms/hooks";
import { toast } from "@vsms/ui";
import { parseAsString, useQueryState } from "nuqs";
import { useEffect, useState } from "react";
import { useForm } from "react-hook-form";
import { ProviderEditDrawer } from "./components/provider-edit-drawer";
import { ProviderQuickDetail } from "./components/provider-quick-detail";
import { ProvidersTable } from "./components/providers-table";
import { ProvidersView } from "./components/providers-view";
import { type EditFormValues, editSchema } from "./edit-schema";

type RouterOutputs = inferRouterOutputs<AppRouter>;
type ProviderListItem = RouterOutputs["providers"]["list"][number];

export function ProvidersScreen() {
  const listQuery = trpc.providers.list.useQuery();
  const utils = trpc.useUtils();

  // Quick detail: local state, no route ownership (D14) — losing it on
  // refresh is fine, reopening is one click on the same row.
  const [quickId, setQuickId] = useState<string | null>(null);
  const quickDetail = listQuery.data?.find((p: ProviderListItem) => p.id === quickId);

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
    <ProvidersView
      errorMessage={listQuery.isError ? listQuery.error.message : null}
      table={
        <ProvidersTable
          rows={listQuery.data ?? []}
          isLoading={listQuery.isLoading}
          onRowClick={setQuickId}
        />
      }
      quickDetail={
        <ProviderQuickDetail
          open={quickId !== null}
          detail={quickDetail}
          onClose={() => setQuickId(null)}
          onEdit={(id) => {
            void setPanelId(id);
            setQuickId(null);
          }}
        />
      }
      editDrawer={
        <ProviderEditDrawer
          open={panelId !== null}
          recordId={panelId}
          onClose={closeMore}
          isLoadingDetail={detailQuery.isLoading}
          detail={detailQuery.data?.data}
          control={form.control}
          register={form.register}
          errors={form.formState.errors}
          onSubmit={form.handleSubmit(onSubmit)}
          isSaving={updateMutation.isPending}
          canSave={detailQuery.data?.etag !== undefined}
          saveError={updateMutation.isError ? updateMutation.error.message : null}
        />
      }
    />
  );
}
