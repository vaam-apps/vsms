"use client";

// The Routes screen (#54): list, plus create/edit/delete — every write real,
// tested code, and (as of #211) real against a real gateway for a
// signed-in `owner`/`admin` — `Route.create`/`update`/`delete`'s own
// `@@allow` is narrower than `Provider.update`'s (`hasRole('owner') ||
// hasRole('admin')` only, no `operator`), so this screen's writes need one
// of those two roles specifically. See `providers-screen.tsx`'s own module
// doc for the mechanism (`resolveUpstreamAccessToken`,
// `packages/gateway/src/request-credential.ts`) — identical here, just a
// narrower Layer 1 gate. A denial surfaces verbatim in whichever drawer's
// own error banner triggered it — never swallowed.
//
// # The zero-routes state gets its own, unmissable banner
//
// §62/#54: a deployment with zero `Route` rows refuses to dispatch every
// message, loudly, not silently. An empty table on this screen is the same
// signal `crates/sms-worker/src/routing.rs::explain_no_route` puts in a
// rejected `Message.stateReason` — surfaced here too, not just discoverable
// after the fact on a rejected message.
//
// # Quick vs. more detail (console-redesign.md §3/D14)
//
// A row click opens `QuickDetailDrawer` (priority/weight/predicates/
// provider — everything already on the list row) with an "Edit" action
// that upgrades to `MoreDetailDrawer`, which owns `?panel=<id>` (or
// `?panel=new` for creation) and holds the real form. Delete is a
// destructive action with real, irreversible consequences (§1.7) — it
// stays a centered `Dialog`, opened from inside the more-detail drawer,
// never a drawer of either weight itself.

import { zodResolver } from "@hookform/resolvers/zod";
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
type RouteListItem = RouterOutputs["routes"]["list"][number];

const OPERATOR_CODES = ["mtn", "orange", "camtel", "nexttel", "unknown"] as const;
const MESSAGE_CLASSES = ["otp", "transactional", "notification", "marketing"] as const;
const ANY = "__any";

function predicateSummary(
  route: Pick<RouteListItem, "matchOperator" | "matchClass" | "matchAppId" | "matchPrefix">,
): string {
  const parts: string[] = [];
  if (route.matchOperator !== undefined) parts.push(`operator=${route.matchOperator}`);
  if (route.matchClass !== undefined) parts.push(`class=${route.matchClass}`);
  if (route.matchAppId !== undefined) parts.push("app-scoped");
  if (route.matchPrefix !== undefined) parts.push(`prefix=${route.matchPrefix}`);
  return parts.length === 0 ? "matches anything" : parts.join(", ");
}

const routeSchema = z.object({
  name: z.string().trim().min(1, "Name is required"),
  priority: z
    .string()
    .trim()
    .refine((v) => Number.isInteger(Number(v)) && Number(v) >= 0 && Number(v) <= 1000, {
      message: "0–1000",
    }),
  weight: z
    .string()
    .trim()
    .refine((v) => Number.isInteger(Number(v)) && Number(v) >= 0 && Number(v) <= 1000, {
      message: "0–1000",
    }),
  enabled: z.enum(["enabled", "disabled"]),
  matchOperator: z.string(),
  matchClass: z.string(),
  matchAppId: z.string(),
  matchPrefix: z.string(),
  providerId: z.string().min(1, "Select a provider"),
});
type RouteFormValues = z.infer<typeof routeSchema>;

const EMPTY_VALUES: RouteFormValues = {
  name: "",
  priority: "0",
  weight: "1",
  enabled: "enabled",
  matchOperator: ANY,
  matchClass: ANY,
  matchAppId: "",
  matchPrefix: "",
  providerId: "",
};

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
    defaultValues: EMPTY_VALUES,
  });

  // biome-ignore lint/correctness/useExhaustiveDependencies: `form` is stable and `isCreate` derives from `panelId` (already a dep) — only re-seed when the target route id or panel mode changes.
  useEffect(() => {
    if (panelId === null) return;
    if (isCreate) {
      form.reset(EMPTY_VALUES);
      return;
    }
    if (editTarget === undefined) return;
    form.reset({
      name: editTarget.name,
      priority: String(editTarget.priority),
      weight: String(editTarget.weight),
      enabled: editTarget.enabled ? "enabled" : "disabled",
      matchOperator: editTarget.matchOperator ?? ANY,
      matchClass: editTarget.matchClass ?? ANY,
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
      setDeleteTarget(null);
      void setPanelId(null);
      void utils.routes.list.invalidate();
    },
  });
  const [deleteTarget, setDeleteTarget] = useState<RouteListItem | null>(null);

  const pendingMutation = isCreate ? createMutation : updateMutation;

  function closeMore() {
    void setPanelId(null);
    createMutation.reset();
    updateMutation.reset();
  }

  function onSubmit(values: RouteFormValues) {
    const fields = {
      name: values.name,
      priority: Number(values.priority),
      weight: Number(values.weight),
      enabled: values.enabled === "enabled",
      ...(values.matchOperator !== ANY
        ? { matchOperator: values.matchOperator as (typeof OPERATOR_CODES)[number] }
        : {}),
      ...(values.matchClass !== ANY
        ? { matchClass: values.matchClass as (typeof MESSAGE_CLASSES)[number] }
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
    <main className="mx-auto flex max-w-[1400px] flex-col gap-6 px-4 py-6 sm:px-6 sm:py-10">
      <header className="flex flex-col gap-1 border-edge border-b pb-6">
        <p className="font-mono text-micro text-subtle-foreground tracking-[0.03em]">
          vsms admin console
        </p>
        <h1 className="font-medium text-foreground text-title">Routes</h1>
        <p className="max-w-xl text-body text-muted-foreground">
          Priority, weight, and match predicates — sorted by priority, highest first.
        </p>
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
        <Button type="button" onClick={() => void setPanelId("new")}>
          New route
        </Button>
      </div>

      <Table>
        <TableHeader>
          <TableRow>
            <TableHead align="end">Priority</TableHead>
            <TableHead align="end" className="hidden sm:table-cell">
              Weight
            </TableHead>
            <TableHead>Status</TableHead>
            <TableHead>Name</TableHead>
            <TableHead className="hidden md:table-cell">Predicates</TableHead>
            <TableHead className="hidden lg:table-cell">Provider</TableHead>
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
                <InlineEmptyState message="No routes configured." />
              </td>
            </tr>
          )}

          {listQuery.data?.map((route: RouteListItem) => (
            <TableRow
              key={route.id}
              className="cursor-pointer"
              onClick={() => setQuickId(route.id)}
            >
              <TableCell align="end" mono>
                {route.priority}
              </TableCell>
              <TableCell align="end" mono className="hidden sm:table-cell">
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
              <TableCell className="hidden text-caption text-muted-foreground md:table-cell">
                {predicateSummary(route)}
              </TableCell>
              <TableCell className="hidden lg:table-cell">
                <IdDisplay value={route.providerId} />
              </TableCell>
              <TableCell align="end" className="hidden md:table-cell">
                <TimestampDisplay value={route.updatedAt} />
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
        {quickDetail !== undefined && (
          <dl className="flex flex-col gap-3 text-body">
            <div className="flex items-center justify-between gap-3">
              <dt className="text-muted-foreground">Status</dt>
              <dd>{quickDetail.enabled ? "enabled" : "disabled"}</dd>
            </div>
            <div className="flex items-center justify-between gap-3">
              <dt className="text-muted-foreground">Priority</dt>
              <dd className="font-mono">{quickDetail.priority}</dd>
            </div>
            <div className="flex items-center justify-between gap-3">
              <dt className="text-muted-foreground">Weight</dt>
              <dd className="font-mono">{quickDetail.weight}</dd>
            </div>
            <div className="flex flex-col gap-1">
              <dt className="text-muted-foreground">Predicates</dt>
              <dd className="text-caption">{predicateSummary(quickDetail)}</dd>
            </div>
            <div className="flex items-center justify-between gap-3">
              <dt className="text-muted-foreground">Provider</dt>
              <dd>
                <IdDisplay value={quickDetail.providerId} variant="full" />
              </dd>
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

      {/* More detail — create or edit, owns `?panel=<id>|new` (D14). */}
      <MoreDetailDrawer
        open={panelId !== null}
        onOpenChange={(open) => !open && closeMore()}
        title={isCreate ? "New route" : (editTarget?.name ?? "Route")}
        description={
          !isCreate &&
          editTarget !== undefined && <IdDisplay value={editTarget.id} variant="full" />
        }
        footer={
          <>
            {!isCreate && editTarget !== undefined && (
              <Button
                type="button"
                variant="destructive"
                size="sm"
                className="mr-auto"
                onClick={() => setDeleteTarget(editTarget)}
              >
                Delete
              </Button>
            )}
            <Button type="button" variant="ghost" onClick={closeMore}>
              Cancel
            </Button>
            <Button type="submit" form="route-edit-form" disabled={pendingMutation.isPending}>
              {pendingMutation.isPending ? "Saving…" : isCreate ? "Create" : "Save"}
            </Button>
          </>
        }
      >
        <form
          id="route-edit-form"
          onSubmit={form.handleSubmit(onSubmit)}
          className="flex flex-col gap-4"
        >
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="route-name">Name</Label>
            <Input
              id="route-name"
              aria-invalid={form.formState.errors.name != null}
              {...form.register("name")}
            />
            {form.formState.errors.name != null && (
              <p className="text-caption text-state-danger-fg">
                {form.formState.errors.name.message}
              </p>
            )}
          </div>

          <div className="grid grid-cols-2 gap-3">
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="route-priority">Priority (0–1000, higher wins)</Label>
              <Input
                id="route-priority"
                inputMode="numeric"
                aria-invalid={form.formState.errors.priority != null}
                {...form.register("priority")}
              />
              {form.formState.errors.priority != null && (
                <p className="text-caption text-state-danger-fg">
                  {form.formState.errors.priority.message}
                </p>
              )}
            </div>
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="route-weight">Weight (within a priority band)</Label>
              <Input
                id="route-weight"
                inputMode="numeric"
                aria-invalid={form.formState.errors.weight != null}
                {...form.register("weight")}
              />
              {form.formState.errors.weight != null && (
                <p className="text-caption text-state-danger-fg">
                  {form.formState.errors.weight.message}
                </p>
              )}
            </div>
          </div>

          <div className="flex flex-col gap-1.5">
            <Label htmlFor="route-enabled">Status</Label>
            <Controller
              control={form.control}
              name="enabled"
              render={({ field }) => (
                <Select value={field.value} onValueChange={field.onChange}>
                  <SelectTrigger id="route-enabled">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="enabled">Enabled</SelectItem>
                    <SelectItem value="disabled">Disabled</SelectItem>
                  </SelectContent>
                </Select>
              )}
            />
          </div>

          <div className="flex flex-col gap-1.5">
            <Label htmlFor="route-provider">Provider</Label>
            <Controller
              control={form.control}
              name="providerId"
              render={({ field }) => (
                <Select value={field.value} onValueChange={field.onChange}>
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
              )}
            />
            {form.formState.errors.providerId != null && (
              <p className="text-caption text-state-danger-fg">
                {form.formState.errors.providerId.message}
              </p>
            )}
          </div>

          <p className="text-caption text-muted-foreground">
            Match predicates below — each left as "any" matches every candidate for that field
            (§6.3: `NULL` on a `match*` column is a wildcard, never "matches nothing").
          </p>

          <div className="grid grid-cols-2 gap-3">
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="route-match-operator">Operator</Label>
              <Controller
                control={form.control}
                name="matchOperator"
                render={({ field }) => (
                  <Select value={field.value} onValueChange={field.onChange}>
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
                )}
              />
            </div>
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="route-match-class">Message class</Label>
              <Controller
                control={form.control}
                name="matchClass"
                render={({ field }) => (
                  <Select value={field.value} onValueChange={field.onChange}>
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
                )}
              />
            </div>
          </div>

          <div className="grid grid-cols-2 gap-3">
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="route-match-app-id">App id</Label>
              <Input id="route-match-app-id" placeholder="any" {...form.register("matchAppId")} />
            </div>
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="route-match-prefix">National prefix</Label>
              <Input
                id="route-match-prefix"
                placeholder="e.g. 677"
                {...form.register("matchPrefix")}
              />
            </div>
          </div>

          {pendingMutation.isError && (
            <div className="rounded-sm border border-state-danger-border bg-state-danger-bg px-3 py-2 text-caption text-state-danger-fg">
              Save failed: {pendingMutation.error.message}
            </div>
          )}
        </form>
      </MoreDetailDrawer>

      {/* Delete confirm — destructive, always a centered Dialog (§1.7/§3),
          never a drawer, opened from inside the more-detail drawer. */}
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
