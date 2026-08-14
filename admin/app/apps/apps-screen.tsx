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
// second portal — see that function's own doc for the full mechanism and
// for why this is a `packages/ui` bug (Phase 1's surface, not this
// bucket's) rather than a design choice made here.
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

import { zodResolver } from "@hookform/resolvers/zod";
import type { inferRouterOutputs } from "@trpc/server";
import type { AppRouter } from "@vsms/api";
import { trpc } from "@vsms/hooks";
import {
  Button,
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  IdDisplay,
  InlineEmptyState,
  Input,
  Label,
  MoreDetailDrawer,
  Skeleton,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
  Textarea,
  TimestampDisplay,
  toast,
} from "@vsms/ui";
import { useQueryState } from "nuqs";
import { useEffect, useState } from "react";
import { useForm } from "react-hook-form";
import { z } from "zod";

type RouterOutputs = inferRouterOutputs<AppRouter>;
type AppListItem = RouterOutputs["apps"]["list"][number];
type AppClientListItem = RouterOutputs["appClients"]["listForApp"][number];

// Mirrors `packages/api/src/routers/apps.ts`'s `createInput`/`updateInput`
// — see that file's own zod schemas, read directly rather than guessed.
const SLUG_PATTERN = /^[a-z0-9][a-z0-9-]{1,38}[a-z0-9]$/;

const appCreateSchema = z.object({
  name: z.string().trim().min(2, "At least 2 characters").max(64, "At most 64 characters"),
  slug: z
    .string()
    .trim()
    .min(2, "At least 2 characters")
    .max(40, "At most 40 characters")
    .regex(SLUG_PATTERN, "lowercase, digits, hyphens — no leading/trailing hyphen"),
  description: z.string().trim().max(500, "At most 500 characters").optional(),
  monthlyQuota: z.number().int("Whole numbers only").nonnegative("Must be zero or more"),
  ipAllowlist: z.string(),
  transliterateToGsm7: z.boolean(),
});
type AppCreateValues = z.infer<typeof appCreateSchema>;

const appEditSchema = appCreateSchema.omit({ slug: true }).extend({ active: z.boolean() });
type AppEditValues = z.infer<typeof appEditSchema>;

const provisionClientSchema = z.object({
  label: z.string().trim().min(1, "Required").max(64, "At most 64 characters"),
  scopes: z.string().trim().min(1, "At least one scope is required"),
});
type ProvisionClientValues = z.infer<typeof provisionClientSchema>;

const APP_CREATE_FIELDS = [
  "name",
  "slug",
  "description",
  "monthlyQuota",
  "ipAllowlist",
  "transliterateToGsm7",
] as const;
function isAppCreateField(field: string): field is (typeof APP_CREATE_FIELDS)[number] {
  return (APP_CREATE_FIELDS as readonly string[]).includes(field);
}

const APP_EDIT_FIELDS = ["name", "description", "monthlyQuota", "ipAllowlist", "active"] as const;
function isAppEditField(field: string): field is (typeof APP_EDIT_FIELDS)[number] {
  return (APP_EDIT_FIELDS as readonly string[]).includes(field);
}

function toIpAllowlistLines(entries: string[]): string {
  return entries.join("\n");
}

function parseIpAllowlistLines(text: string): string[] {
  return text
    .split(/\r?\n|,/)
    .map((line) => line.trim())
    .filter((line) => line.length > 0);
}

function ErrorBanner({ children }: { children: React.ReactNode }) {
  return (
    <div className="rounded-sm border border-state-danger-border bg-state-danger-bg px-3 py-2 text-caption text-state-danger-fg">
      {children}
    </div>
  );
}

// **Not a `Dialog`, on purpose — found live, not a style preference.**
// `AppClientsPanel` (below) only ever renders while nested inside
// `AppDetailDrawer`'s own `MoreDetailDrawer`. `packages/ui/src/components/
// primitives/drawer.tsx`'s own doc already states, in writing, that vaul
// never forwards its `modal` prop down to the Radix `Dialog` underneath
// it — "Radix's own focus trap... [is] unconditional either way." Nesting
// a second, independently-portaled Headless UI `Dialog` (a *second* focus
// trap, implemented by a different library, with no awareness of the
// first) inside that always-on trap was verified live, in this browser,
// to self-dismiss the entire drawer within roughly half a second of
// opening — with zero further clicks or keystrokes from the operator: the
// two focus scopes fight over where focus is allowed to live, and vaul's
// own dismissable-layer reads the resulting focus jump as "the user
// clicked outside," closing the drawer out from under the dialog. This is
// exactly the risk docs/design/console-redesign.md §8 names ("a nested
// case... needs its z-index layered above the drawer that opened it...
// Phase 3 must test the nested case") — reproduced here ahead of that
// phase, for real, not merely predicted. `packages/ui` is Phase 1's own
// surface, not this bucket's, so the fix belongs there; until it lands,
// every confirmation/form flow inside a drawer in this screen renders
// inline instead of through a second portal — no second focus trap, no
// conflict, and it still satisfies §3's own "show it once, plainly"
// bar for a secret this sensitive.
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
    // Clears the mutation hook's own held `data` (the private key) —
    // see this screen's own module doc.
    provisionMutation.reset();
    form.reset();
    onOpenChange(false);
  }

  const key = provisionMutation.data;

  function onSubmit(values: ProvisionClientValues) {
    provisionMutation.mutate({
      appId,
      label: values.label,
      scopes: values.scopes.split(/\s+/).filter((s) => s.length > 0),
    });
  }

  if (!open) return null;

  return (
    <div className="flex flex-col gap-4 rounded-sm border border-edge bg-surface-2 p-4">
      <div>
        <h4 className="font-medium text-body text-foreground">
          Provision a service-account client
        </h4>
        <p className="mt-1 text-caption text-muted-foreground">
          The private key is shown exactly once. It is never stored anywhere by this console or by
          sms-api — copy it now, or the client has to be retired and re-provisioned.
        </p>
      </div>

      {key === undefined && (
        <form
          id="provision-client-form"
          onSubmit={form.handleSubmit(onSubmit)}
          className="flex flex-col gap-4"
        >
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="client-label">Label</Label>
            <Input
              id="client-label"
              placeholder="e.g. billing-service"
              aria-invalid={form.formState.errors.label != null}
              {...form.register("label")}
            />
            {form.formState.errors.label != null && (
              <p className="text-caption text-state-danger-fg">
                {form.formState.errors.label.message}
              </p>
            )}
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="client-scopes">Scopes (space-separated)</Label>
            <Input
              id="client-scopes"
              aria-invalid={form.formState.errors.scopes != null}
              {...form.register("scopes")}
            />
            {form.formState.errors.scopes != null ? (
              <p className="text-caption text-state-danger-fg">
                {form.formState.errors.scopes.message}
              </p>
            ) : (
              <p className="text-caption text-subtle-foreground">
                e.g. <span className="font-mono">sms:send sms:read</span>
              </p>
            )}
          </div>
          {provisionMutation.isError && (
            <ErrorBanner>{provisionMutation.error.message}</ErrorBanner>
          )}

          <div className="flex items-center justify-end gap-2">
            <Button type="button" variant="ghost" onClick={closeAndClear}>
              Cancel
            </Button>
            <Button type="submit" disabled={provisionMutation.isPending}>
              {provisionMutation.isPending ? "Provisioning…" : "Provision"}
            </Button>
          </div>
        </form>
      )}

      {key !== undefined && (
        <div className="flex flex-col gap-3">
          <div className="rounded-sm border border-edge bg-surface-1 px-3 py-2 text-caption text-muted-foreground">
            Client id: <span className="font-mono text-foreground">{key.clientId}</span>
          </div>
          <div className="flex flex-col gap-1.5">
            <Label>Private key (PKCS#8 PEM) — save this now</Label>
            <Textarea
              readOnly
              rows={10}
              className="font-mono text-caption"
              value={key.privateKeyPem}
            />
            <div className="flex items-center gap-2">
              <Button
                type="button"
                variant="secondary"
                size="sm"
                onClick={() => {
                  void navigator.clipboard.writeText(key.privateKeyPem);
                  toast({ title: "Private key copied", variant: "success" });
                }}
              >
                Copy key
              </Button>
            </div>
          </div>
          <div className="flex justify-end">
            <Button type="button" onClick={closeAndClear}>
              I&apos;ve saved this key — close
            </Button>
          </div>
        </div>
      )}
    </div>
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

  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h3 className="font-medium text-body text-foreground">Service-account clients</h3>
        <Button type="button" size="sm" onClick={() => setProvisionOpen(true)}>
          Provision client
        </Button>
      </div>

      {listQuery.isError && <ErrorBanner>{listQuery.error.message}</ErrorBanner>}

      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>Label</TableHead>
            <TableHead className="hidden sm:table-cell">Client id</TableHead>
            <TableHead className="hidden md:table-cell">Scopes</TableHead>
            <TableHead>Active</TableHead>
            <TableHead align="end">Actions</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {listQuery.isLoading && (
            <TableRow>
              <TableCell colSpan={5}>
                <Skeleton className="h-4 w-full" />
              </TableCell>
            </TableRow>
          )}
          {!listQuery.isLoading && (listQuery.data?.length ?? 0) === 0 && (
            <TableRow>
              <TableCell colSpan={5}>
                <InlineEmptyState message="No clients provisioned for this app yet." />
              </TableCell>
            </TableRow>
          )}
          {listQuery.data?.map((client: AppClientListItem) => (
            <TableRow key={client.id}>
              <TableCell>{client.label}</TableCell>
              <TableCell mono className="hidden sm:table-cell">
                <IdDisplay value={client.clientId} />
              </TableCell>
              <TableCell mono className="hidden text-caption md:table-cell">
                {client.scopes.trim()}
              </TableCell>
              <TableCell>
                {client.active ? (
                  <span className="text-state-success-fg">active</span>
                ) : (
                  <span className="text-muted-foreground">retired</span>
                )}
              </TableCell>
              <TableCell align="end">
                {client.active && (
                  <Button
                    type="button"
                    variant="ghost"
                    size="sm"
                    onClick={() => setRetiringId(client.id)}
                  >
                    Retire
                  </Button>
                )}
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>

      {/* Inline, not a `Dialog` — see `ProvisionClientPanel`'s own doc for
       * why: this whole panel only ever renders nested inside an already-
       * open `MoreDetailDrawer`, where a second, independently-portaled
       * focus trap was verified live to self-dismiss the drawer. */}
      {retiringId !== null && (
        <div className="flex flex-col gap-3 rounded-sm border border-state-danger-border bg-state-danger-bg p-4">
          <div>
            <p className="font-medium text-body text-state-danger-fg">Retire this client?</p>
            <p className="mt-1 text-caption text-state-danger-fg">
              This is immediate and total — there is no overlap window. The client&apos;s current
              key stops authenticating the instant this succeeds. If a live integration still uses
              it, provision its replacement and migrate first.
            </p>
          </div>
          <div className="flex justify-end gap-2">
            <Button type="button" variant="ghost" onClick={() => setRetiringId(null)}>
              Cancel
            </Button>
            <Button
              type="button"
              variant="destructive"
              disabled={retireMutation.isPending}
              onClick={() => {
                const client = listQuery.data?.find((c) => c.id === retiringId);
                if (client === undefined) return;
                retireMutation.mutate({ id: client.id, etag: String(client.version) });
              }}
            >
              {retireMutation.isPending ? "Retiring…" : "Retire client"}
            </Button>
          </div>
        </div>
      )}

      <ProvisionClientPanel appId={appId} open={provisionOpen} onOpenChange={setProvisionOpen} />
    </div>
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
    <Dialog
      open={open}
      onOpenChange={(next) => {
        if (!next) form.reset();
        onOpenChange(next);
      }}
    >
      <DialogContent className="max-w-[480px]">
        <DialogHeader>
          <DialogTitle>New app</DialogTitle>
        </DialogHeader>
        <form
          id="create-app-form"
          onSubmit={form.handleSubmit(onSubmit)}
          className="flex flex-col gap-4"
        >
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="app-name">Name</Label>
            <Input
              id="app-name"
              aria-invalid={form.formState.errors.name != null}
              {...form.register("name")}
            />
            {form.formState.errors.name != null && (
              <p className="text-caption text-state-danger-fg">
                {form.formState.errors.name.message}
              </p>
            )}
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="app-slug">Slug</Label>
            <Input
              id="app-slug"
              placeholder="lowercase-with-hyphens"
              aria-invalid={form.formState.errors.slug != null}
              {...form.register("slug")}
            />
            {form.formState.errors.slug != null && (
              <p className="text-caption text-state-danger-fg">
                {form.formState.errors.slug.message}
              </p>
            )}
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="app-quota">Monthly quota</Label>
            <Input
              id="app-quota"
              type="number"
              min="0"
              aria-invalid={form.formState.errors.monthlyQuota != null}
              {...form.register("monthlyQuota", { valueAsNumber: true })}
            />
            {form.formState.errors.monthlyQuota != null && (
              <p className="text-caption text-state-danger-fg">
                {form.formState.errors.monthlyQuota.message}
              </p>
            )}
          </div>
          {generalError != null && <ErrorBanner>{generalError}</ErrorBanner>}
        </form>
        <DialogFooter>
          <Button type="button" variant="ghost" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button type="submit" form="create-app-form" disabled={createMutation.isPending}>
            {createMutation.isPending ? "Creating…" : "Create"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
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

  return (
    <MoreDetailDrawer
      open={open}
      onOpenChange={(next) => !next && onClose()}
      title={detailQuery.data?.data?.name ?? "App"}
      description={appId !== null && <IdDisplay value={appId} variant="full" />}
      footer={
        <>
          <Button
            type="button"
            variant="destructive"
            size="sm"
            className="mr-auto"
            onClick={() => setDeleteConfirmOpen(true)}
          >
            Delete app
          </Button>
          <Button type="button" variant="ghost" onClick={onClose}>
            Close
          </Button>
          <Button
            type="submit"
            form="app-edit-form"
            disabled={updateMutation.isPending || detailQuery.data === undefined}
          >
            {updateMutation.isPending ? "Saving…" : "Save"}
          </Button>
        </>
      }
    >
      {detailQuery.isLoading && <Skeleton className="h-32 w-full" />}
      {detailQuery.isError && (
        <ErrorBanner>Could not read this app: {detailQuery.error.message}</ErrorBanner>
      )}

      {appId !== null && detailQuery.data?.data !== undefined && (
        <div className="flex flex-col gap-6">
          <form
            id="app-edit-form"
            onSubmit={form.handleSubmit(onSubmit)}
            className="flex flex-col gap-4"
          >
            <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
              <div className="flex flex-col gap-1.5">
                <Label htmlFor="app-edit-name">Name</Label>
                <Input
                  id="app-edit-name"
                  aria-invalid={form.formState.errors.name != null}
                  {...form.register("name")}
                />
                {form.formState.errors.name != null && (
                  <p className="text-caption text-state-danger-fg">
                    {form.formState.errors.name.message}
                  </p>
                )}
              </div>
              <div className="flex flex-col gap-1.5">
                <Label>Slug</Label>
                <Input value={detailQuery.data.data.slug} disabled />
              </div>
            </div>

            <div className="flex flex-col gap-1.5">
              <Label htmlFor="app-edit-description">Description</Label>
              <Textarea id="app-edit-description" rows={2} {...form.register("description")} />
            </div>

            <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
              <div className="flex flex-col gap-1.5">
                <Label htmlFor="app-edit-quota">Monthly quota</Label>
                <Input
                  id="app-edit-quota"
                  type="number"
                  min="0"
                  aria-invalid={form.formState.errors.monthlyQuota != null}
                  {...form.register("monthlyQuota", { valueAsNumber: true })}
                />
                {form.formState.errors.monthlyQuota != null && (
                  <p className="text-caption text-state-danger-fg">
                    {form.formState.errors.monthlyQuota.message}
                  </p>
                )}
              </div>
              <div className="flex items-end gap-4 pb-2">
                <label className="flex items-center gap-2 text-caption text-foreground">
                  <input
                    type="checkbox"
                    className="checkbox checkbox-sm"
                    {...form.register("transliterateToGsm7")}
                  />
                  Transliterate to GSM-7
                </label>
                <label className="flex items-center gap-2 text-caption text-foreground">
                  <input
                    type="checkbox"
                    className="checkbox checkbox-sm"
                    {...form.register("active")}
                  />
                  Active
                </label>
              </div>
            </div>

            <div className="flex flex-col gap-1.5">
              <Label htmlFor="app-edit-allowlist">
                IP allowlist (one CIDR per line — blank = unrestricted)
              </Label>
              <Textarea
                id="app-edit-allowlist"
                rows={3}
                className="font-mono text-caption"
                {...form.register("ipAllowlist")}
              />
            </div>

            {isStale && (
              <div className="flex items-center justify-between gap-3 rounded-sm border border-state-warning-border bg-state-warning-bg px-3 py-2 text-caption text-state-warning-fg">
                <span>
                  Someone else changed this app since it loaded. Reload to see their edit.
                </span>
                <Button
                  type="button"
                  variant="secondary"
                  size="sm"
                  onClick={() => void detailQuery.refetch()}
                >
                  Reload
                </Button>
              </div>
            )}
            {generalError != null && <ErrorBanner>Save failed: {generalError}</ErrorBanner>}
          </form>

          {/* Inline, not a `Dialog` — see `ProvisionClientPanel`'s own doc
           * above (`AppClientsPanel`) for why: this drawer is already
           * open, and a second, independently-portaled focus trap nested
           * inside it was verified live to self-dismiss the whole drawer
           * within about half a second, with no further interaction. */}
          {deleteConfirmOpen && (
            <div className="flex flex-col gap-3 rounded-sm border border-state-danger-border bg-state-danger-bg p-4">
              <div>
                <p className="font-medium text-body text-state-danger-fg">Delete this app?</p>
                <p className="mt-1 text-caption text-state-danger-fg">
                  This soft-deletes the row (owner only) — existing messages and clients referencing
                  it are untouched, but the app stops being usable for new sends.
                </p>
              </div>
              <div className="flex justify-end gap-2">
                <Button type="button" variant="ghost" onClick={() => setDeleteConfirmOpen(false)}>
                  Cancel
                </Button>
                <Button
                  type="button"
                  variant="destructive"
                  disabled={deleteMutation.isPending}
                  onClick={() => deleteMutation.mutate({ id: appId })}
                >
                  {deleteMutation.isPending ? "Deleting…" : "Delete"}
                </Button>
              </div>
            </div>
          )}

          <div className="border-edge border-t pt-4">
            <AppClientsPanel appId={appId} />
          </div>
        </div>
      )}
    </MoreDetailDrawer>
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
    <div className="flex flex-col gap-6">
      <div className="flex flex-col items-start justify-between gap-4 border-edge border-b pb-6 sm:flex-row sm:items-center">
        <div>
          <h1 className="font-medium text-foreground text-title">Apps</h1>
          <p className="mt-1 max-w-xl text-body text-muted-foreground">
            Every integrated product, its quota, and its service-account clients.
          </p>
        </div>
        <Button type="button" onClick={() => setCreateOpen(true)} className="shrink-0">
          New app
        </Button>
      </div>

      <div className="rounded-sm border border-edge bg-surface-2 px-3 py-2 text-caption text-muted-foreground">
        Reads and writes act as you — saving requires your own role to carry{" "}
        <span className="font-mono text-foreground">app:write</span> (owner and admin by default),
        and provisioning/retiring a client needs{" "}
        <span className="font-mono text-foreground">user:manage</span>-adjacent trust: this
        console&apos;s own permission table gates it at{" "}
        <span className="font-mono text-foreground">owner</span>/
        <span className="font-mono text-foreground">admin</span> only.
      </div>

      {listQuery.isError && (
        <ErrorBanner>Could not read apps: {listQuery.error.message}</ErrorBanner>
      )}

      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>Active</TableHead>
            <TableHead>Name</TableHead>
            <TableHead className="hidden sm:table-cell">Slug</TableHead>
            <TableHead align="end" className="hidden md:table-cell">
              Monthly quota
            </TableHead>
            <TableHead className="hidden lg:table-cell">Transliterate to GSM-7</TableHead>
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
                <TableCell colSpan={6}>
                  <Skeleton className="h-4 w-full" />
                </TableCell>
              </TableRow>
            ))}

          {!listQuery.isLoading && (listQuery.data?.length ?? 0) === 0 && (
            <TableRow>
              <TableCell colSpan={6}>
                <InlineEmptyState message="No apps yet." />
              </TableCell>
            </TableRow>
          )}

          {listQuery.data?.map((app: AppListItem) => (
            <TableRow
              key={app.id}
              className="cursor-pointer"
              onClick={() => void setPanelId(app.id)}
            >
              <TableCell>
                {app.active ? (
                  <span className="text-state-success-fg">yes</span>
                ) : (
                  <span className="text-muted-foreground">no</span>
                )}
              </TableCell>
              <TableCell>{app.name}</TableCell>
              <TableCell mono className="hidden sm:table-cell">
                {app.slug}
              </TableCell>
              <TableCell align="end" mono className="hidden md:table-cell">
                {app.monthlyQuota.toLocaleString()}
              </TableCell>
              <TableCell className="hidden lg:table-cell">
                {app.transliterateToGsm7 ? "on" : "off"}
              </TableCell>
              <TableCell align="end" className="hidden md:table-cell">
                <TimestampDisplay value={app.updatedAt} />
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>

      <CreateAppDialog open={createOpen} onOpenChange={setCreateOpen} />

      <AppDetailDrawer
        appId={stickyPanelId}
        open={panelId !== null}
        onClose={() => void setPanelId(null)}
      />
    </div>
  );
}
