"use client";

// The Routes screen (#54): list, plus create/edit/delete — every write real,
// tested code, and (as of #211) real against a real gateway for a
// signed-in `owner`/`admin` — `Route.create`/`update`/`delete`'s own
// `@@allow` is narrower than `Provider.update`'s (`hasRole('owner') ||
// hasRole('admin')` only, no `operator`), so this screen's writes need one
// of those two roles specifically. See `providers-screen.tsx`'s own module
// doc for the mechanism (`resolveUpstreamAccessToken`,
// `packages/gateway/src/request-credential.ts`) — identical here, just a
// narrower Layer 1 gate.
//
// # The zero-routes state gets its own, unmissable banner
//
// §62/#54: a deployment with zero `Route` rows refuses to dispatch every
// message, loudly, not silently. An empty table on this screen is the same
// signal `crates/sms-worker/src/routing.rs::explain_no_route` puts in a
// rejected `Message.stateReason` — surfaced here too, not just discoverable
// after the fact on a rejected message.

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
  TimestampDisplay,
  toast,
} from "@vsms/ui";
import { useState } from "react";

type RouterOutputs = inferRouterOutputs<AppRouter>;
type RouteListItem = RouterOutputs["routes"]["list"][number];

const OPERATOR_CODES = ["mtn", "orange", "camtel", "nexttel", "unknown"] as const;
const MESSAGE_CLASSES = ["otp", "transactional", "notification", "marketing"] as const;
const ANY = "__any";

interface RouteFormState {
  name: string;
  priority: string;
  weight: string;
  enabled: boolean;
  matchOperator: string;
  matchClass: string;
  matchAppId: string;
  matchPrefix: string;
  providerId: string;
}

const EMPTY_FORM: RouteFormState = {
  name: "",
  priority: "0",
  weight: "1",
  enabled: true,
  matchOperator: ANY,
  matchClass: ANY,
  matchAppId: "",
  matchPrefix: "",
  providerId: "",
};

function predicateSummary(route: RouteListItem): string {
  const parts: string[] = [];
  if (route.matchOperator !== undefined) parts.push(`operator=${route.matchOperator}`);
  if (route.matchClass !== undefined) parts.push(`class=${route.matchClass}`);
  if (route.matchAppId !== undefined) parts.push("app-scoped");
  if (route.matchPrefix !== undefined) parts.push(`prefix=${route.matchPrefix}`);
  return parts.length === 0 ? "matches anything" : parts.join(", ");
}

export function RoutesScreen() {
  const listQuery = trpc.routes.list.useQuery();
  const providersQuery = trpc.providers.list.useQuery();
  const utils = trpc.useUtils();

  const [editTarget, setEditTarget] = useState<RouteListItem | "new" | null>(null);
  const [form, setForm] = useState<RouteFormState>(EMPTY_FORM);
  const [deleteTarget, setDeleteTarget] = useState<RouteListItem | null>(null);

  const createMutation = trpc.routes.create.useMutation({
    onSuccess: () => {
      toast({ title: "Route created", variant: "success" });
      setEditTarget(null);
      void utils.routes.list.invalidate();
    },
  });
  const updateMutation = trpc.routes.update.useMutation({
    onSuccess: () => {
      toast({ title: "Route saved", variant: "success" });
      setEditTarget(null);
      void utils.routes.list.invalidate();
    },
  });
  const deleteMutation = trpc.routes.remove.useMutation({
    onSuccess: () => {
      toast({ title: "Route deleted", variant: "success" });
      setDeleteTarget(null);
      void utils.routes.list.invalidate();
    },
  });

  function openCreate() {
    setForm(EMPTY_FORM);
    createMutation.reset();
    setEditTarget("new");
  }

  function openEdit(route: RouteListItem) {
    setForm({
      name: route.name,
      priority: String(route.priority),
      weight: String(route.weight),
      enabled: route.enabled,
      matchOperator: route.matchOperator ?? ANY,
      matchClass: route.matchClass ?? ANY,
      matchAppId: route.matchAppId ?? "",
      matchPrefix: route.matchPrefix ?? "",
      providerId: route.providerId,
    });
    updateMutation.reset();
    setEditTarget(route);
  }

  function closeDialog() {
    setEditTarget(null);
  }

  function submit() {
    const fields = {
      name: form.name,
      priority: Number(form.priority),
      weight: Number(form.weight),
      enabled: form.enabled,
      ...(form.matchOperator !== ANY
        ? { matchOperator: form.matchOperator as (typeof OPERATOR_CODES)[number] }
        : {}),
      ...(form.matchClass !== ANY
        ? { matchClass: form.matchClass as (typeof MESSAGE_CLASSES)[number] }
        : {}),
      ...(form.matchAppId !== "" ? { matchAppId: form.matchAppId } : {}),
      ...(form.matchPrefix !== "" ? { matchPrefix: form.matchPrefix } : {}),
      providerId: form.providerId,
    };

    if (editTarget === "new") {
      createMutation.mutate(fields);
    } else if (editTarget !== null) {
      // Synthesized, not fetched fresh: `RouteListItem.version` already
      // carries what this operator last observed, and the server's own
      // `ETag` is exactly `"<version>"` (`rest.ts`'s own doc). Building it
      // from the list row is correct, not a shortcut — a stale list means a
      // stale `If-Match`, which is precisely what should 412, the same
      // outcome a fresh `GET` immediately before this `PATCH` would produce.
      updateMutation.mutate({ id: editTarget.id, etag: `"${editTarget.version}"`, ...fields });
    }
  }

  const pendingMutation = editTarget === "new" ? createMutation : updateMutation;

  return (
    <main className="mx-auto flex max-w-[1400px] flex-col gap-6 px-6 py-10">
      <header className="flex items-start justify-between gap-4 border-edge border-b pb-6">
        <div>
          <p className="font-mono text-micro text-subtle-foreground tracking-[0.03em]">
            vsms admin console
          </p>
          <h1 className="mt-1 font-medium text-foreground text-title">Routes</h1>
          <p className="mt-1 max-w-xl text-body text-muted-foreground">
            Priority, weight, and match predicates — sorted by priority, highest first.
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
            href="/providers"
            className="text-caption text-muted-foreground underline decoration-edge-strong underline-offset-2 hover:decoration-foreground"
          >
            Providers
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
          <a
            href="/sender-ids"
            className="text-caption text-muted-foreground underline decoration-edge-strong underline-offset-2 hover:decoration-foreground"
          >
            Sender IDs
          </a>
          <a
            href="/webhooks"
            className="text-caption text-muted-foreground underline decoration-edge-strong underline-offset-2 hover:decoration-foreground"
          >
            Webhooks
          </a>
        </div>
      </header>

      <div className="rounded-sm border border-edge bg-surface-2 px-3 py-2 text-caption text-muted-foreground">
        Create/Save/Delete act as you, not as a shared service account — they require your own role
        to be <span className="font-mono text-foreground">owner</span> or{" "}
        <span className="font-mono text-foreground">admin</span>; other roles (including operator)
        will see a real <span className="font-mono text-foreground">Forbidden</span> here.
      </div>

      {!listQuery.isLoading && (listQuery.data?.length ?? 0) === 0 && (
        <div className="rounded-sm border border-state-danger-border bg-state-danger-bg px-3 py-2 text-caption text-state-danger-fg">
          No routes configured at all — every message this system accepts is refused, loudly (§62).
          At least one enabled route is required before anything can be dispatched.
        </div>
      )}

      {listQuery.isError && (
        <div className="rounded-sm border border-state-danger-border bg-state-danger-bg px-3 py-2 text-caption text-state-danger-fg">
          Could not read routes: {listQuery.error.message}
        </div>
      )}

      <div>
        <Button type="button" onClick={openCreate}>
          New route
        </Button>
      </div>

      <Table>
        <TableHeader>
          <TableRow>
            <TableHead align="end">Priority</TableHead>
            <TableHead align="end">Weight</TableHead>
            <TableHead>Enabled</TableHead>
            <TableHead>Name</TableHead>
            <TableHead>Predicates</TableHead>
            <TableHead>Provider</TableHead>
            <TableHead align="end">Updated</TableHead>
            <TableHead align="end">Actions</TableHead>
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
                <InlineEmptyState message="No routes configured." />
              </td>
            </tr>
          )}

          {listQuery.data?.map((route: RouteListItem) => (
            <TableRow key={route.id}>
              <TableCell align="end" mono>
                {route.priority}
              </TableCell>
              <TableCell align="end" mono>
                {route.weight}
              </TableCell>
              <TableCell>
                {route.enabled ? (
                  <span className="rounded-sm border border-state-success-border bg-state-success-bg px-1.5 py-0.5 text-caption text-state-success-fg">
                    enabled
                  </span>
                ) : (
                  <span className="rounded-sm border border-state-danger-border bg-state-danger-bg px-1.5 py-0.5 text-caption text-state-danger-fg">
                    disabled
                  </span>
                )}
              </TableCell>
              <TableCell>{route.name}</TableCell>
              <TableCell className="text-caption text-muted-foreground">
                {predicateSummary(route)}
              </TableCell>
              <TableCell>
                <IdDisplay value={route.providerId} />
              </TableCell>
              <TableCell align="end">
                <TimestampDisplay value={route.updatedAt} />
              </TableCell>
              <TableCell align="end">
                <div className="flex justify-end gap-2">
                  <Button
                    type="button"
                    variant="secondary"
                    size="sm"
                    onClick={() => openEdit(route)}
                  >
                    Edit
                  </Button>
                  <Button
                    type="button"
                    variant="secondary"
                    size="sm"
                    onClick={() => setDeleteTarget(route)}
                  >
                    Delete
                  </Button>
                </div>
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>

      <Dialog open={editTarget !== null} onOpenChange={(open) => !open && closeDialog()}>
        <DialogContent className="max-w-[560px]">
          <DialogHeader>
            <DialogTitle>{editTarget === "new" ? "New route" : "Edit route"}</DialogTitle>
            <DialogDescription>
              {editTarget !== null && editTarget !== "new" && (
                <IdDisplay value={editTarget.id} variant="full" />
              )}
            </DialogDescription>
          </DialogHeader>

          <div className="flex flex-col gap-4">
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="route-name">Name</Label>
              <Input
                id="route-name"
                value={form.name}
                onChange={(e) => setForm({ ...form, name: e.target.value })}
              />
            </div>

            <div className="grid grid-cols-2 gap-3">
              <div className="flex flex-col gap-1.5">
                <Label htmlFor="route-priority">Priority (0–1000, higher wins)</Label>
                <Input
                  id="route-priority"
                  type="number"
                  min="0"
                  max="1000"
                  step="1"
                  value={form.priority}
                  onChange={(e) => setForm({ ...form, priority: e.target.value })}
                />
              </div>
              <div className="flex flex-col gap-1.5">
                <Label htmlFor="route-weight">Weight (0–1000, within a priority band)</Label>
                <Input
                  id="route-weight"
                  type="number"
                  min="0"
                  max="1000"
                  step="1"
                  value={form.weight}
                  onChange={(e) => setForm({ ...form, weight: e.target.value })}
                />
              </div>
            </div>

            <div className="flex flex-col gap-1.5">
              <Label htmlFor="route-enabled">Status</Label>
              <Select
                value={form.enabled ? "enabled" : "disabled"}
                onValueChange={(value) => setForm({ ...form, enabled: value === "enabled" })}
              >
                <SelectTrigger id="route-enabled">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="enabled">Enabled</SelectItem>
                  <SelectItem value="disabled">Disabled</SelectItem>
                </SelectContent>
              </Select>
            </div>

            <div className="flex flex-col gap-1.5">
              <Label htmlFor="route-provider">Provider</Label>
              <Select
                value={form.providerId}
                onValueChange={(value) => setForm({ ...form, providerId: value })}
              >
                <SelectTrigger id="route-provider">
                  <SelectValue placeholder="Select a provider" />
                </SelectTrigger>
                <SelectContent>
                  {providersQuery.data?.map((provider) => (
                    <SelectItem key={provider.id} value={provider.id}>
                      {provider.displayName} ({provider.key})
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>

            <p className="text-caption text-muted-foreground">
              Match predicates below — each left as "any" matches every candidate for that field
              (§6.3: `NULL` on a `match*` column is a wildcard, never "matches nothing").
            </p>

            <div className="grid grid-cols-2 gap-3">
              <div className="flex flex-col gap-1.5">
                <Label htmlFor="route-match-operator">Operator</Label>
                <Select
                  value={form.matchOperator}
                  onValueChange={(value) => setForm({ ...form, matchOperator: value })}
                >
                  <SelectTrigger id="route-match-operator">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value={ANY}>Any</SelectItem>
                    {OPERATOR_CODES.map((code) => (
                      <SelectItem key={code} value={code}>
                        {code}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
              <div className="flex flex-col gap-1.5">
                <Label htmlFor="route-match-class">Message class</Label>
                <Select
                  value={form.matchClass}
                  onValueChange={(value) => setForm({ ...form, matchClass: value })}
                >
                  <SelectTrigger id="route-match-class">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value={ANY}>Any</SelectItem>
                    {MESSAGE_CLASSES.map((cls) => (
                      <SelectItem key={cls} value={cls}>
                        {cls}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
            </div>

            <div className="grid grid-cols-2 gap-3">
              <div className="flex flex-col gap-1.5">
                <Label htmlFor="route-match-app-id">App id</Label>
                <Input
                  id="route-match-app-id"
                  placeholder="any"
                  value={form.matchAppId}
                  onChange={(e) => setForm({ ...form, matchAppId: e.target.value })}
                />
              </div>
              <div className="flex flex-col gap-1.5">
                <Label htmlFor="route-match-prefix">National prefix</Label>
                <Input
                  id="route-match-prefix"
                  placeholder="e.g. 677"
                  value={form.matchPrefix}
                  onChange={(e) => setForm({ ...form, matchPrefix: e.target.value })}
                />
              </div>
            </div>

            {pendingMutation.isError && (
              <div className="rounded-sm border border-state-danger-border bg-state-danger-bg px-3 py-2 text-caption text-state-danger-fg">
                Save failed: {pendingMutation.error.message}
              </div>
            )}
          </div>

          <DialogFooter>
            <Button type="button" variant="ghost" onClick={closeDialog}>
              Cancel
            </Button>
            <Button
              type="button"
              disabled={pendingMutation.isPending || form.providerId === ""}
              onClick={submit}
            >
              {pendingMutation.isPending ? "Saving…" : editTarget === "new" ? "Create" : "Save"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog open={deleteTarget !== null} onOpenChange={(open) => !open && setDeleteTarget(null)}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Delete this route?</DialogTitle>
            <DialogDescription>
              {deleteTarget !== null && (
                <>
                  <span className="font-mono text-foreground">{deleteTarget.name}</span> will be
                  removed permanently. This cannot be undone.
                </>
              )}
            </DialogDescription>
          </DialogHeader>
          {deleteMutation.isError && (
            <div className="rounded-sm border border-state-danger-border bg-state-danger-bg px-3 py-2 text-caption text-state-danger-fg">
              Delete failed: {deleteMutation.error.message}
            </div>
          )}
          <DialogFooter>
            <Button type="button" variant="ghost" onClick={() => setDeleteTarget(null)}>
              Cancel
            </Button>
            <Button
              type="button"
              variant="destructive"
              disabled={deleteMutation.isPending}
              onClick={() =>
                deleteTarget !== null && deleteMutation.mutate({ id: deleteTarget.id })
              }
            >
              Delete
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </main>
  );
}
