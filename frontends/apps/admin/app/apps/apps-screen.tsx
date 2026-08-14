"use client";

// The Apps screen (#52): apps, their service-account clients, quota, and
// the ipAllowlist/transliterateToGsm7 toggles — plus #211's own real
// reads-and-writes-as-you proof case, the same shape `providers-screen.tsx`
// already established. `App.update`/`App.delete`'s own `@@allow`
// (`owner`/`admin` only, `App.delete` `owner`-only) admits no
// `auth().kind == "app"` clause, so this screen's writes only became
// reachable once #211 forwarded the signed-in human's own session token.
//
// # Console redesign (Phase 2, Admin group) — what changed and why
//
// Row click no longer opens a centered `Dialog` — it opens a
// **`MoreDetailDrawer`** (docs/design/console-redesign.md §3/D14): the
// edit form plus the nested service-account-client table is exactly "the
// full record: every field, an edit form, ... nested history if it's
// short," and the existing `providers-screen.tsx` edit dialog is D14's own
// named precedent for this exact conversion. No intermediate
// `QuickDetailDrawer` peek was added in front of it — the design doc lists
// Providers/Routes/Sender IDs/Jobs/Opt-outs as quick-detail candidates and
// is silent on Apps, and going straight from a row click to the full
// record matches how `providers-screen.tsx` itself works, not a shortcut
// taken here. The drawer owns a shallow `?panel=<appId>` route (`nuqs`,
// `history: "replace"` so opening/closing rows doesn't spam browser
// history) per D14's own "survives refresh and is linkable" requirement —
// this is a genuine, testable difference from the old `Dialog`, which lost
// the open record on every refresh.
//
// Every form on this screen (`New app`, the app edit form, `Provision
// client`) is now `react-hook-form` + `zod` (#236), reusing the exact
// `error.data?.fieldErrors` → `form.setError(field, { type: "server" })`
// wiring `app/page.tsx`'s composer already established — a 422 from
// sms-api lands on the specific field, not a generic banner. Client-side
// zod bounds mirror `packages/api/src/routers/apps.ts`'s own input
// schemas (read, not guessed); the server remains the actual source of
// truth for anything this client-side copy gets wrong or falls behind on.
//
// # "Show secret once" — what this screen actually does about it
//
// `provisionAppClient` returns `privateKeyPem` in exactly one response.
// [`ProvisionClientPanel`] renders it from the mutation's own `data` (a
// `useMutation` hook's result lives in that hook's component-local React
// state, not TanStack Query's shared cache — nothing else in this app can
// read it back), never writes it into `localStorage`/`sessionStorage`,
// never routes it through a toast (`toast()` calls here are for outcomes
// only — "client provisioned", never the key itself), and calling
// `provisionMutation.reset()` on close clears the hook's own held `data`
// immediately rather than leaving it retrievable by, say, a browser
// back/forward cache restoring the component. The panel requires an
// explicit "I've saved this key — close" click (not a bare "Close") so
// dismissing it isn't a single careless click. **Not a centered `Dialog`,
// though §3's own rule would otherwise put a show-once secret there** — a
// real, framework-level finding overrides that here: `packages/ui`'s
// `drawer.tsx` never disables Radix's underlying focus trap regardless of
// `dimmed` (its own doc says so explicitly), so a second, independently-
// portaled Headless UI `Dialog` nested inside this screen's already-open
// `MoreDetailDrawer` was verified live, in a real browser, to self-dismiss
// the whole drawer within about half a second — no user interaction
// beyond the initial trigger click required. `ProvisionClientPanel`
// therefore renders inline, inside the drawer's own content, never a
// second portal — see `./components/provision-client-panel-view.tsx` for
// the markup and `AppClientsPanelView` for how the two compose.
//
// # Retire — the coarse fallback, stated on screen, not just in a comment
//
// There is no per-client key-history model, so retiring is immediate and
// total: the moment it succeeds, the old key stops authenticating. See
// `@vsms/gateway/app-clients.ts`'s own module doc for the full reasoning
// and what a caller wanting zero-downtime rotation has to do instead
// (provision the replacement first, migrate the integration, retire the
// old one last) — this screen's own copy says the same thing next to the
// button, not just in source.
//
// # Stale writes (`412`) are surfaced, not swallowed
//
// A save that lost the optimistic-concurrency race comes back as tRPC's
// `CONFLICT` code (sms-api's `409`/`412` both map onto it — see
// `@vsms/gateway/errors.ts`'s own doc for why a client component can't
// distinguish the two more precisely than that). Either way the row
// changed under the operator; [`AppDetailDrawer`] shows a dedicated
// "someone else changed this" banner with a one-click reload instead of
// the generic error text, and a `412` is exercised for real in
// `docs/design/console-redesign.md`'s own read of `crates/sms-api/tests/
// if_match_live_postgres.rs` (server-side) — this screen's job is only to
// not hide the outcome.
//
// # R6 — layer split
//
// All markup and classes now live in `./components/*`, dumb "view"
// components that receive data and callbacks and know nothing about tRPC.
// This file keeps the smart orchestration functions it already had
// (`ProvisionClientPanel`, `AppClientsPanel`, `CreateAppDialog`,
// `AppDetailDrawer`, `AppsScreen`) — each one now does data fetching,
// mutations and derived values only, and renders exactly one dumb view.
// `SLUG_PATTERN`/the zod schemas/the field-shape guards moved to
// `./app-forms.ts`; `toIpAllowlistLines`/`parseIpAllowlistLines` moved to
// `./ip-allowlist.ts` (both pure, both carry tests, per R6's "extracted
// pure modules carry tests").

import { zodResolver } from "@hookform/resolvers/zod";
import { trpc } from "@vsms/hooks";
import { ScreenStack, toast } from "@vsms/ui";
import { useQueryState } from "nuqs";
import { useEffect, useState } from "react";
import { useForm } from "react-hook-form";
import {
  type AppCreateValues,
  type AppEditValues,
  appCreateSchema,
  appEditSchema,
  isAppCreateField,
  isAppEditField,
  type ProvisionClientValues,
  provisionClientSchema,
} from "./app-forms";
import { AppClientsPanelView } from "./components/app-clients-panel-view";
import { AppDetailDrawerView } from "./components/app-detail-drawer-view";
import { AppsHeader } from "./components/apps-header";
import { AppsTable } from "./components/apps-table";
import { CreateAppDialogView } from "./components/create-app-dialog-view";
import { ErrorBanner } from "./components/error-banner";
import { InlineConfirmPanel } from "./components/inline-confirm-panel";
import { ProvisionClientPanelView } from "./components/provision-client-panel-view";
import { parseIpAllowlistLines, toIpAllowlistLines } from "./ip-allowlist";
import type { AppClientListItem, AppListItem } from "./types";

function ProvisionClientPanel({
  appId,
  open,
  onOpenChange,
}: {
  appId: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const utils = trpc.useUtils();
  const form = useForm<ProvisionClientValues>({
    resolver: zodResolver(provisionClientSchema),
    defaultValues: { label: "", scopes: "sms:send sms:read" },
  });
  const provisionMutation = trpc.appClients.provision.useMutation({
    onSuccess: () => {
      void utils.appClients.listForApp.invalidate({ appId });
    },
    onError: (error) => {
      const fieldErrors = error.data?.fieldErrors;
      if (fieldErrors == null) return;
      for (const [field, messages] of Object.entries(fieldErrors)) {
        if (field === "label" || field === "scopes") {
          const msg = messages[0];
          if (msg != null) form.setError(field, { type: "server", message: msg });
        }
      }
    },
  });

  function closeAndClear() {
    // Clears the mutation hook's own held `data` (the private key) — see
    // this screen's own module doc.
    provisionMutation.reset();
    form.reset();
    onOpenChange(false);
  }

  function onSubmit(values: ProvisionClientValues) {
    provisionMutation.mutate({
      appId,
      label: values.label,
      scopes: values.scopes.split(/\s+/).filter((s) => s.length > 0),
    });
  }

  return (
    <ProvisionClientPanelView
      open={open}
      form={form}
      onSubmit={onSubmit}
      onCancel={closeAndClear}
      onDone={closeAndClear}
      isPending={provisionMutation.isPending}
      isError={provisionMutation.isError}
      errorMessage={provisionMutation.error?.message ?? ""}
      result={provisionMutation.data}
    />
  );
}

function AppClientsPanel({ appId }: { appId: string }) {
  const listQuery = trpc.appClients.listForApp.useQuery({ appId });
  const utils = trpc.useUtils();
  const [provisionOpen, setProvisionOpen] = useState(false);
  const [retiringId, setRetiringId] = useState<string | null>(null);

  const retireMutation = trpc.appClients.retire.useMutation({
    onSuccess: () => {
      toast({ title: "Client retired", variant: "success" });
      setRetiringId(null);
      void utils.appClients.listForApp.invalidate({ appId });
    },
  });

  const retiringClient = listQuery.data?.find((c: AppClientListItem) => c.id === retiringId);

  return (
    <AppClientsPanelView
      clients={listQuery.data ?? []}
      isLoading={listQuery.isLoading}
      errorMessage={listQuery.isError ? listQuery.error.message : null}
      onProvisionClick={() => setProvisionOpen(true)}
      onRetireClick={(client) => setRetiringId(client.id)}
    >
      {retiringId !== null && (
        <InlineConfirmPanel
          title="Retire this client?"
          description="This is immediate and total — there is no overlap window. The client's current key stops authenticating the instant this succeeds. If a live integration still uses it, provision its replacement and migrate first."
          confirmLabel="Retire client"
          pendingLabel="Retiring…"
          pending={retireMutation.isPending}
          onCancel={() => setRetiringId(null)}
          onConfirm={() => {
            if (retiringClient === undefined) return;
            retireMutation.mutate({ id: retiringClient.id, etag: String(retiringClient.version) });
          }}
        />
      )}

      <ProvisionClientPanel appId={appId} open={provisionOpen} onOpenChange={setProvisionOpen} />
    </AppClientsPanelView>
  );
}

function CreateAppDialog({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const utils = trpc.useUtils();
  const form = useForm<AppCreateValues>({
    resolver: zodResolver(appCreateSchema),
    defaultValues: {
      name: "",
      slug: "",
      description: "",
      monthlyQuota: 10000,
      ipAllowlist: "",
      transliterateToGsm7: false,
    },
  });
  const createMutation = trpc.apps.create.useMutation({
    onSuccess: () => {
      toast({ title: "App created", variant: "success" });
      form.reset();
      onOpenChange(false);
      void utils.apps.list.invalidate();
    },
    onError: (error) => {
      const fieldErrors = error.data?.fieldErrors;
      if (fieldErrors == null) return;
      for (const [field, messages] of Object.entries(fieldErrors)) {
        const msg = messages[0];
        if (isAppCreateField(field) && msg != null) {
          form.setError(field, { type: "server", message: msg });
        }
      }
    },
  });

  function onSubmit(values: AppCreateValues) {
    createMutation.mutate({
      name: values.name,
      slug: values.slug,
      description: values.description === "" ? undefined : values.description,
      monthlyQuota: values.monthlyQuota,
      ipAllowlist: parseIpAllowlistLines(values.ipAllowlist),
      transliterateToGsm7: values.transliterateToGsm7,
    });
  }

  const hasFieldErrors = createMutation.error?.data?.fieldErrors != null;
  const generalError =
    createMutation.isError && !hasFieldErrors ? createMutation.error.message : null;

  return (
    <CreateAppDialogView
      open={open}
      onOpenChange={(next) => {
        if (!next) form.reset();
        onOpenChange(next);
      }}
      form={form}
      onSubmit={onSubmit}
      isPending={createMutation.isPending}
      generalError={generalError}
    />
  );
}

// `appId`/`open` are separate on purpose — see this file's own
// `useStickyId` doc for why. The drawer is always mounted (`AppsScreen`
// never conditionally renders this component), so `vaul`'s own close
// transition gets a chance to play: unmounting the whole `Drawer.Root`
// the instant `open` flips `false` would remove it from the DOM before a
// single animation frame runs, and the panel would just vanish instead of
// sliding out. `id` is nullable so this can render (closed, with a
// generic "App" title) before any row has ever been opened.
function AppDetailDrawer({
  appId,
  open,
  onClose,
}: {
  appId: string | null;
  open: boolean;
  onClose: () => void;
}) {
  const utils = trpc.useUtils();
  const [deleteConfirmOpen, setDeleteConfirmOpen] = useState(false);

  const detailQuery = trpc.apps.get.useQuery({ id: appId ?? "" }, { enabled: appId !== null });
  const form = useForm<AppEditValues>({
    resolver: zodResolver(appEditSchema),
    defaultValues: {
      name: "",
      description: "",
      monthlyQuota: 0,
      ipAllowlist: "",
      transliterateToGsm7: false,
      active: true,
    },
  });

  useEffect(() => {
    const d = detailQuery.data?.data;
    if (d === undefined) return;
    form.reset({
      name: d.name,
      description: d.description ?? "",
      monthlyQuota: d.monthlyQuota,
      ipAllowlist: toIpAllowlistLines(
        d.ipAllowlist
          .trim()
          .split(/\s+/)
          .filter((e) => e.length > 0),
      ),
      transliterateToGsm7: d.transliterateToGsm7,
      active: d.active,
    });
    // `form.reset` is a stable reference (react-hook-form memoizes its
    // returned methods), so listing it here satisfies the lint without
    // ever causing an extra reset — the effect still only genuinely
    // re-runs when `detailQuery.data` itself changes.
  }, [detailQuery.data, form.reset]);

  const updateMutation = trpc.apps.update.useMutation({
    onSuccess: () => {
      toast({ title: "App saved", variant: "success" });
      void utils.apps.list.invalidate();
      if (appId !== null) void utils.apps.get.invalidate({ id: appId });
    },
    onError: (error) => {
      const fieldErrors = error.data?.fieldErrors;
      if (fieldErrors == null) return;
      for (const [field, messages] of Object.entries(fieldErrors)) {
        const msg = messages[0];
        if (isAppEditField(field) && msg != null)
          form.setError(field, { type: "server", message: msg });
      }
    },
  });

  const deleteMutation = trpc.apps.delete.useMutation({
    onSuccess: () => {
      toast({ title: "App deleted", variant: "success" });
      setDeleteConfirmOpen(false);
      void utils.apps.list.invalidate();
      onClose();
    },
  });

  function onSubmit(values: AppEditValues) {
    const etag = detailQuery.data?.etag;
    if (etag === undefined || appId === null) return;
    updateMutation.mutate({
      id: appId,
      etag,
      name: values.name,
      description: values.description === "" ? undefined : values.description,
      monthlyQuota: values.monthlyQuota,
      ipAllowlist: parseIpAllowlistLines(values.ipAllowlist),
      transliterateToGsm7: values.transliterateToGsm7,
      active: values.active,
    });
  }

  const isStale = updateMutation.error?.data?.code === "CONFLICT";
  const hasFieldErrors = updateMutation.error?.data?.fieldErrors != null;
  const generalError =
    updateMutation.isError && !isStale && !hasFieldErrors ? updateMutation.error.message : null;
  const detail = detailQuery.data?.data;

  return (
    <AppDetailDrawerView
      appId={appId}
      open={open}
      onOpenChange={(next) => !next && onClose()}
      title={detail?.name ?? "App"}
      isLoading={detailQuery.isLoading}
      loadError={detailQuery.isError ? detailQuery.error.message : null}
      hasDetail={detail !== undefined}
      form={form}
      slug={detail?.slug ?? ""}
      onSubmit={onSubmit}
      isStale={isStale}
      onReload={() => void detailQuery.refetch()}
      generalError={generalError}
      isSaving={updateMutation.isPending}
      onDeleteClick={() => setDeleteConfirmOpen(true)}
      onClose={onClose}
      deleteConfirm={
        deleteConfirmOpen &&
        appId !== null && (
          <InlineConfirmPanel
            title="Delete this app?"
            description="This soft-deletes the row (owner only) — existing messages and clients referencing it are untouched, but the app stops being usable for new sends."
            confirmLabel="Delete"
            pendingLabel="Deleting…"
            pending={deleteMutation.isPending}
            onCancel={() => setDeleteConfirmOpen(false)}
            onConfirm={() => deleteMutation.mutate({ id: appId })}
          />
        )
      }
      clientsPanel={appId !== null && <AppClientsPanel appId={appId} />}
    />
  );
}

export function AppsScreen() {
  const listQuery = trpc.apps.list.useQuery();
  const [createOpen, setCreateOpen] = useState(false);
  // D14: the more-details drawer owns a shallow `?panel=<id>` route so it
  // survives a refresh and is linkable — `history: "replace"` because
  // opening/closing a row is a peek-adjacent action, not a navigation
  // worth a back-button stop (matching `jobs-screen.tsx`'s own filter
  // state, which uses `push` for genuinely different reasons — a changed
  // filter is a different view worth returning to).
  const [panelId, setPanelId] = useQueryState("panel", { history: "replace" });
  // `AppDetailDrawer` stays mounted at all times (rendered unconditionally
  // below) rather than only while `panelId !== null` — a drawer's own
  // closing transition (`vaul`) needs at least one frame with `open=false`
  // still in the DOM to animate; unmounting the whole component the
  // instant a row closes would skip that frame and make the panel vanish
  // instead of sliding out. `stickyPanelId` keeps the last real id on
  // screen through that close animation (so the drawer's content doesn't
  // blank out mid-transition either) and only updates forward, never back
  // to `null` — the same "a stale value beats a flash of nothing" bias
  // `app/page.tsx`'s own `EncodingPreview` `placeholderData` already uses.
  const [stickyPanelId, setStickyPanelId] = useState<string | null>(null);
  useEffect(() => {
    if (panelId !== null) setStickyPanelId(panelId);
  }, [panelId]);

  return (
    <ScreenStack>
      <AppsHeader onCreateClick={() => setCreateOpen(true)} />

      {listQuery.isError && (
        <ErrorBanner>Could not read apps: {listQuery.error.message}</ErrorBanner>
      )}

      <AppsTable
        apps={listQuery.data ?? []}
        isLoading={listQuery.isLoading}
        onRowClick={(app: AppListItem) => void setPanelId(app.id)}
      />

      <CreateAppDialog open={createOpen} onOpenChange={setCreateOpen} />

      <AppDetailDrawer
        appId={stickyPanelId}
        open={panelId !== null}
        onClose={() => void setPanelId(null)}
      />
    </ScreenStack>
  );
}
