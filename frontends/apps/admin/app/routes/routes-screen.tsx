"use client";

// The Routes screen (#54): list, plus create/edit/delete — every write real,
// tested code, and (as of #211) real against a real gateway for a
// signed-in `owner`/`admin` — `Route.create`/`update`/`delete`'s own
// `@@allow` is narrower than `Provider.update`'s (`hasRole('owner') ||
// hasRole('admin')` only, no `operator`), so this screen's writes need one
// of those two roles specifically. See `providers-screen.tsx`'s own module
// `frontends/packages/gateway/src/request-credential.ts`) — identical here, just a
// narrower Layer 1 gate. A denial surfaces verbatim in whichever drawer's
// own error banner triggered it — never swallowed.
//
// # The zero-routes state gets its own, unmissable banner
//
// §62/#54: a deployment with zero `Route` rows refuses to dispatch every
// message, loudly, not silently. An empty table on this screen is the same
// signal `backends/crates/sms-worker/src/routing.rs::explain_no_route` puts in a
// rejected `Message.stateReason` — surfaced here too, not just discoverable
// after the fact on a rejected message.
//
// # Quick vs. more detail (console-redesign.md §3/D14)
//
// A row click opens `QuickDetailDrawer` (priority/weight/predicates/
// provider — everything already on the list row) with an "Edit" action
// that upgrades to `MoreDetailDrawer`, which owns `?panel=<id>` (or
// `?panel=new` for creation) and holds the real form. Delete is a
// destructive action with real, irreversible consequences (§1.7) —
// rendered **inline inside `MoreDetailDrawer`'s own body** (see
// `RouteDeleteConfirm`'s own doc comment), not as a nested `Dialog`: a
// centered `Dialog` nested inside an open `vaul` drawer never becomes
// visible or interactive, a real, verified bug — see
// `frontends/apps/admin/app/gallery/page.tsx`'s
// `NestedDialogInDrawerRegression` demo and `docs/design/console-redesign.md`
// §3/§1.7 for the mechanism.
//
// # R6
//
// This file holds data fetching, mutations, URL/local state, and handlers
// only. Markup and classes live in `./components/*` (route-local — nothing
// here is reused by another screen) and `./route-domain.ts` (the pure form
// schema/predicate-summary logic, extracted so it's unit-testable without
// mounting React).

import { zodResolver } from "@hookform/resolvers/zod";
import { trpc } from "@vsms/hooks";
import {
  Button,
  IdDisplay,
  type MessageClass,
  MoreDetailDrawer,
  QuickDetailDrawer,
  ScreenHeader,
  ScreenShell,
  toast,
} from "@vsms/ui";
import { parseAsString, useQueryState } from "nuqs";
import { useEffect, useState } from "react";
import { useForm } from "react-hook-form";
import { RouteDeleteConfirm } from "./components/route-delete-confirm";
import { RouteEditFooter } from "./components/route-edit-footer";
import { RouteForm } from "./components/route-form";
import { RouteQuickDetailBody } from "./components/route-quick-detail-body";
import { RouteTable } from "./components/route-table";
import { RouteToolbar } from "./components/route-toolbar";
import {
  ANY_PREDICATE,
  EMPTY_ROUTE_FORM_VALUES,
  type OperatorCode,
  type RouteFormValues,
  routeSchema,
} from "./route-domain";

export function RoutesScreen() {
  const listQuery = trpc.routes.list.useQuery();
  const providersQuery = trpc.providers.list.useQuery();
  const utils = trpc.useUtils();

  const [quickId, setQuickId] = useState<string | null>(null);
  const quickDetail = listQuery.data?.find((r) => r.id === quickId);

  // More detail owns `?panel=<id>` (edit) or `?panel=new` (create).
  const [panelId, setPanelId] = useQueryState("panel", parseAsString);
  const isCreate = panelId === "new";
  const editTarget = !isCreate ? listQuery.data?.find((r) => r.id === panelId) : undefined;

  const form = useForm<RouteFormValues>({
    resolver: zodResolver(routeSchema),
    defaultValues: EMPTY_ROUTE_FORM_VALUES,
  });

  // biome-ignore lint/correctness/useExhaustiveDependencies: `form` is stable and `isCreate` derives from `panelId` (already a dep) — only re-seed when the target route id or panel mode changes.
  useEffect(() => {
    if (panelId === null) return;
    if (isCreate) {
      form.reset(EMPTY_ROUTE_FORM_VALUES);
      return;
    }
    if (editTarget === undefined) return;
    form.reset({
      name: editTarget.name,
      priority: String(editTarget.priority),
      weight: String(editTarget.weight),
      enabled: editTarget.enabled ? "enabled" : "disabled",
      matchOperator: editTarget.matchOperator ?? ANY_PREDICATE,
      matchClass: editTarget.matchClass ?? ANY_PREDICATE,
      matchAppId: editTarget.matchAppId ?? "",
      matchPrefix: editTarget.matchPrefix ?? "",
      providerId: editTarget.providerId,
    });
  }, [panelId, editTarget]);

  const createMutation = trpc.routes.create.useMutation({
    onSuccess: () => {
      toast({ title: "Route created", variant: "success" });
      void setPanelId(null);
      void utils.routes.list.invalidate();
    },
  });
  const updateMutation = trpc.routes.update.useMutation({
    onSuccess: () => {
      toast({ title: "Route saved", variant: "success" });
      void setPanelId(null);
      void utils.routes.list.invalidate();
    },
  });
  const deleteMutation = trpc.routes.remove.useMutation({
    onSuccess: () => {
      toast({ title: "Route deleted", variant: "success" });
      setDeleteTargetId(null);
      void setPanelId(null);
      void utils.routes.list.invalidate();
    },
  });
  // An id, not a copy of the row (R6) — `deleteTarget` is derived from the
  // live query below, the same "keep the id, look the row up" shape `panel`
  // already uses.
  const [deleteTargetId, setDeleteTargetId] = useState<string | null>(null);
  const deleteTarget = listQuery.data?.find((r) => r.id === deleteTargetId);

  const pendingMutation = isCreate ? createMutation : updateMutation;

  function closeMore() {
    void setPanelId(null);
    setDeleteTargetId(null);
    createMutation.reset();
    updateMutation.reset();
  }

  function onSubmit(values: RouteFormValues) {
    const fields = {
      name: values.name,
      priority: Number(values.priority),
      weight: Number(values.weight),
      enabled: values.enabled === "enabled",
      ...(values.matchOperator !== ANY_PREDICATE
        ? { matchOperator: values.matchOperator as OperatorCode }
        : {}),
      ...(values.matchClass !== ANY_PREDICATE
        ? { matchClass: values.matchClass as MessageClass }
        : {}),
      ...(values.matchAppId !== "" ? { matchAppId: values.matchAppId } : {}),
      ...(values.matchPrefix !== "" ? { matchPrefix: values.matchPrefix } : {}),
      providerId: values.providerId,
    };

    if (isCreate) {
      createMutation.mutate(fields);
    } else if (editTarget !== undefined) {
      // Synthesized, not fetched fresh: `RouteListItem.version` already
      // carries what this operator last observed, and the server's own
      // `ETag` is exactly `"<version>"`. A stale list means a stale
      // `If-Match`, which is precisely what should 412.
      updateMutation.mutate({ id: editTarget.id, etag: `"${editTarget.version}"`, ...fields });
    }
  }

  return (
    <ScreenShell>
      <ScreenHeader
        title="Routes"
        description="Priority, weight, and match predicates — sorted by priority, highest first."
      />

      <RouteToolbar
        isEmpty={!listQuery.isLoading && (listQuery.data?.length ?? 0) === 0}
        listErrorMessage={listQuery.error?.message}
        onNewRoute={() => void setPanelId("new")}
      />

      <RouteTable
        routes={listQuery.data}
        isLoading={listQuery.isLoading}
        onRowClick={(r) => setQuickId(r.id)}
      />

      {/* Quick detail — a peek, no route, closes back to exactly where the
          list was. */}
      <QuickDetailDrawer
        open={quickId !== null}
        onOpenChange={(open) => !open && setQuickId(null)}
        title={quickDetail?.name ?? "Route"}
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
        {quickDetail !== undefined && <RouteQuickDetailBody route={quickDetail} />}
      </QuickDetailDrawer>

      {/* More detail — create or edit, owns `?panel=<id>|new` (D14). Its
          body and footer swap to the inline delete confirmation when one is
          armed — see `RouteDeleteConfirm`'s own doc comment for why this is
          not a nested `Dialog`. */}
      <MoreDetailDrawer
        open={panelId !== null}
        onOpenChange={(open) => !open && closeMore()}
        title={isCreate ? "New route" : (editTarget?.name ?? "Route")}
        description={
          !isCreate &&
          editTarget !== undefined && <IdDisplay value={editTarget.id} variant="full" />
        }
        footer={
          deleteTarget !== undefined ? undefined : (
            <RouteEditFooter
              showDelete={!isCreate && editTarget !== undefined}
              isCreate={isCreate}
              pending={pendingMutation.isPending}
              onDelete={() => editTarget !== undefined && setDeleteTargetId(editTarget.id)}
              onCancel={closeMore}
            />
          )
        }
      >
        {deleteTarget !== undefined ? (
          <RouteDeleteConfirm
            route={deleteTarget}
            pending={deleteMutation.isPending}
            errorMessage={deleteMutation.error?.message}
            onConfirm={() => deleteMutation.mutate({ id: deleteTarget.id })}
            onCancel={() => setDeleteTargetId(null)}
          />
        ) : (
          <RouteForm
            formId="route-edit-form"
            form={form}
            providers={providersQuery.data}
            onSubmit={onSubmit}
            saveErrorMessage={pendingMutation.error?.message}
          />
        )}
      </MoreDetailDrawer>
    </ScreenShell>
  );
}
