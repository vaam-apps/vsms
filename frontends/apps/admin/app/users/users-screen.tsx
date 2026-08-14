"use client";

// The Users & Roles screen (#58): console accounts, their roles, and the
// permission sets those roles carry. Two tabs over one screen — assigning
// a role to a user needs the role list right there, and #58 groups them as
// one story.
//
// # Console redesign (Phase 2, Admin group) — what changed and why
//
// Both tabs follow `apps-screen.tsx`'s own precedent (see that file's
// module doc for the full reasoning, not repeated per file): a table row
// click opens a `MoreDetailDrawer` (docs/design/console-redesign.md
// §3/D14) instead of a centered `Dialog`, directly — no intermediate
// `QuickDetailDrawer` peek, matching `providers-screen.tsx`'s own D14
// precedent. The drawer owns a shallow route, but the route now carries
// *two* pieces of state, not one: `?tab=users|roles` (so a deep link lands
// on the right tab, and a refresh doesn't silently snap back to `users`)
// and `?panel=<id>`, scoped implicitly by whichever tab is active —
// Headless UI's `TabGroup` (via `ValueTabs`) unmounts the inactive panel
// by default, so `UsersTab`/`RolesTab` never both hold a `panel` value at
// once in practice, and a stale `panel` left over from a tab switch simply
// matches nothing in the newly-mounted tab's own list. Every form (`New
// role`, `Provision user`, both edit forms) is `react-hook-form` + `zod`
// (#236), reusing the same `fieldErrors` → `form.setError` wiring
// `app/page.tsx`'s composer and `apps-screen.tsx` already established.
//
// # Provisioning shows a password exactly once, the same discipline
// # `apps-screen.tsx` already established for a client's private key
//
// `provisionUser`'s response is read straight from the mutation hook's own
// `data` (component-local state, not the shared query cache), never
// written to storage, never routed through a toast, and
// `provisionMutation.reset()` on close clears it from memory rather than
// leaving it retrievable. See `apps-screen.tsx`'s own module doc — the
// mechanism is identical, applied to a password instead of a PEM. This
// stays a centered `Dialog`, not a drawer, for the identical §3 reason —
// reading a shown-once secret and confirming it's saved is a yes/no
// interaction with no sub-navigation.
//
// # No password rotation
//
// There is no "reset this user's password" action anywhere on this screen
// — no such procedure exists. `backends/crates/sms-api/src/procedures.rs` has no
// write path to `UserCredential` other than `provisionUser`'s own
// once-only create, and there is no way to target an *existing* row with
// it. A locked-out account has no self-service recovery today; see
// `OPEN_QUESTIONS.md` for this named, not silently absent, gap.
//
// # Reserved role keys fail gracefully, not just quietly
//
// `isReservedRoleKey`/`isValidRoleKeyShape` (`@vsms/gateway`) block
// `system`/`app` in the create form before a request is ever sent — the
// database's own `roles_key_not_reserved_check` and
// `sms_api::auth::RESERVED_ROLE_KEYS` remain the real guards regardless
// (`backends/crates/sms-api/src/auth.rs`'s own doc), this is only the friendly
// half — now enforced as a `zod` `.refine()` on the create form's own
// schema rather than a disabled-submit-button check computed separately.
//
// # Delete confirmations render inline, not as a nested `Dialog`
//
// Both `UserDetailDrawer` and `RoleDetailDrawer`'s own delete confirmation
// render inline inside the drawer's body rather than through a second,
// portaled `Dialog` — see `apps-screen.tsx`'s `ProvisionClientPanel` doc
// for the live-verified reason (a second focus trap nested inside an
// already-open `MoreDetailDrawer` self-dismisses the whole drawer). Unlike
// `apps-screen.tsx`'s provision flow, `ProvisionUserDialog` above stays a
// real `Dialog` — it is triggered from `UsersTab`'s own top-level button,
// never from inside an open drawer (the drawer's overlay blocks that
// button while open), so it never hits the conflict.

import { zodResolver } from "@hookform/resolvers/zod";
import type { inferRouterOutputs } from "@trpc/server";
import type { AppRouter } from "@vsms/api";
import { trpc } from "@vsms/hooks";
import {
  Badge,
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
  // D18: `Tabs` was rebuilt as `ValueTabs` (Headless UI `TabGroup` behind a
  // value-based adapter) — aliased on import so the JSX below is untouched.
  ValueTabs as Tabs,
  ValueTabsContent as TabsContent,
  ValueTabsList as TabsList,
  ValueTabsTrigger as TabsTrigger,
  Textarea,
  TimestampDisplay,
  toast,
} from "@vsms/ui";
import { parseAsStringEnum, useQueryState } from "nuqs";
import { useEffect, useState } from "react";
import { Controller, useForm } from "react-hook-form";
import { z } from "zod";

// `@vsms/gateway`'s own `isReservedRoleKey`/`isValidRoleKeyShape` live in
// `roles.ts`, which is `import "server-only"` at the top of the file —
// re-exporting them through `@vsms/gateway`'s index does not strip that
// marker, so importing them here (a `"use client"` component) would fail
// the build. These two are trivial, pure regex checks with no security
// weight of their own (the database `CHECK` and `RESERVED_ROLE_KEYS` in
// `backends/crates/sms-api/src/auth.rs` are the real guards regardless — see this
// file's own module doc), so they're duplicated locally rather than
// pulling a new shared, client-safe package into existence for two
// one-line functions.
const RESERVED_ROLE_KEYS = new Set(["system", "app"]);
const ROLE_KEY_PATTERN = /^[a-z][a-z0-9_]{2,31}$/;

type RouterOutputs = inferRouterOutputs<AppRouter>;
type UserListItem = RouterOutputs["users"]["list"][number];
type RoleRecord = RouterOutputs["roles"]["list"][number];

const KNOWN_PERMISSIONS = [
  "sms:read",
  "sms:send",
  "message:cancel",
  "app:read",
  "app:write",
  "client:provision",
  "provider:read",
  "provider:update",
  "provider:delete",
  "route:read",
  "route:write",
  "sender:manage",
  "optout:manage",
  "webhook:manage",
  "job:read",
  "job:enqueue",
  "worker:read",
  "dashboard:read",
  "audit:read",
  "user:manage",
  "user:delete",
  "role:manage",
];

// Mirrors `packages/api/src/routers/users.ts`/`roles.ts` — read, not
// guessed.
const provisionUserSchema = z.object({
  email: z.string().trim().email("Enter a valid email address"),
  displayName: z.string().trim().min(1, "Required").max(128, "At most 128 characters"),
  roleKey: z.string().min(1, "Pick a role"),
});
type ProvisionUserValues = z.infer<typeof provisionUserSchema>;

const userEditSchema = z.object({
  displayName: z.string().trim().min(1, "Required").max(128, "At most 128 characters"),
  roleKey: z.string().min(1, "Pick a role"),
  active: z.boolean(),
});
type UserEditValues = z.infer<typeof userEditSchema>;

const roleKeySchema = z
  .string()
  .trim()
  .regex(
    ROLE_KEY_PATTERN,
    "Must start with a letter, 3-32 chars, lowercase letters/digits/underscore only",
  )
  .refine((key) => !RESERVED_ROLE_KEYS.has(key), {
    message: '"system" and "app" are reserved and can never be assigned to a role',
  });

const roleCreateSchema = z.object({
  key: roleKeySchema,
  label: z.string().trim().min(2, "At least 2 characters").max(64, "At most 64 characters"),
  permissions: z.string(),
});
type RoleCreateValues = z.infer<typeof roleCreateSchema>;

const roleEditSchema = z.object({
  label: z.string().trim().min(2, "At least 2 characters").max(64, "At most 64 characters"),
  permissions: z.string(),
});
type RoleEditValues = z.infer<typeof roleEditSchema>;

function ErrorBanner({ children }: { children: React.ReactNode }) {
  return (
    <div className="rounded-sm border border-state-danger-border bg-state-danger-bg px-3 py-2 text-caption text-state-danger-fg">
      {children}
    </div>
  );
}

function StaleWriteBanner({ onReload }: { onReload: () => void }) {
  return (
    <div className="flex items-center justify-between gap-3 rounded-sm border border-state-warning-border bg-state-warning-bg px-3 py-2 text-caption text-state-warning-fg">
      <span>Someone else changed this row since it loaded. Reload to see their edit.</span>
      <Button type="button" variant="secondary" size="sm" onClick={onReload}>
        Reload
      </Button>
    </div>
  );
}

function ProvisionUserDialog({
  open,
  onOpenChange,
  roles,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  roles: RoleRecord[];
}) {
  const utils = trpc.useUtils();
  const form = useForm<ProvisionUserValues>({
    resolver: zodResolver(provisionUserSchema),
    defaultValues: { email: "", displayName: "", roleKey: roles[0]?.key ?? "" },
  });
  const provisionMutation = trpc.users.provision.useMutation({
    onSuccess: () => {
      void utils.users.list.invalidate();
    },
    onError: (error) => {
      const fieldErrors = error.data?.fieldErrors;
      if (fieldErrors == null) return;
      for (const [field, messages] of Object.entries(fieldErrors)) {
        if (field === "email" || field === "displayName" || field === "roleKey") {
          const msg = messages[0];
          if (msg != null) form.setError(field, { type: "server", message: msg });
        }
      }
    },
  });

  function closeAndClear() {
    provisionMutation.reset();
    form.reset();
    onOpenChange(false);
  }

  const result = provisionMutation.data;

  function onSubmit(values: ProvisionUserValues) {
    provisionMutation.mutate(values);
  }

  return (
    <Dialog open={open} onOpenChange={(next) => (next ? undefined : closeAndClear())}>
      <DialogContent className="max-w-[480px]">
        <DialogHeader>
          <DialogTitle>Provision a console account</DialogTitle>
          <DialogDescription>
            The one-time password is shown exactly once — copy it now, or the account has to be
            deactivated and provisioned again under a different email.
          </DialogDescription>
        </DialogHeader>

        {result === undefined && (
          <form
            id="provision-user-form"
            onSubmit={form.handleSubmit(onSubmit)}
            className="flex flex-col gap-4"
          >
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="user-email">Email</Label>
              <Input
                id="user-email"
                type="email"
                aria-invalid={form.formState.errors.email != null}
                {...form.register("email")}
              />
              {form.formState.errors.email != null && (
                <p className="text-caption text-state-danger-fg">
                  {form.formState.errors.email.message}
                </p>
              )}
            </div>
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="user-display-name">Display name</Label>
              <Input
                id="user-display-name"
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
              <Label htmlFor="user-role">Role</Label>
              <Controller
                control={form.control}
                name="roleKey"
                render={({ field }) => (
                  <Select value={field.value} onValueChange={field.onChange}>
                    <SelectTrigger id="user-role">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {roles.map((role) => (
                        <SelectItem key={role.key} value={role.key}>
                          {role.label} ({role.key})
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                )}
              />
            </div>
            {provisionMutation.isError && (
              <ErrorBanner>{provisionMutation.error.message}</ErrorBanner>
            )}
          </form>
        )}

        {result !== undefined && (
          <div className="flex flex-col gap-3">
            <div className="rounded-sm border border-edge bg-surface-2 px-3 py-2 text-caption text-muted-foreground">
              {result.email} — role{" "}
              <span className="font-mono text-foreground">{result.roleKey}</span>
            </div>
            <div className="flex flex-col gap-1.5">
              <Label>One-time password — save this now</Label>
              <div className="flex items-center gap-2">
                <Input readOnly className="font-mono" value={result.password} />
                <Button
                  type="button"
                  variant="secondary"
                  size="sm"
                  onClick={() => {
                    void navigator.clipboard.writeText(result.password);
                    toast({ title: "Password copied", variant: "success" });
                  }}
                >
                  Copy
                </Button>
              </div>
              <p className="text-caption text-subtle-foreground">
                Share this over a channel the recipient controls, not this screen&apos;s own log.
              </p>
            </div>
          </div>
        )}

        <DialogFooter>
          {result === undefined ? (
            <>
              <Button type="button" variant="ghost" onClick={closeAndClear}>
                Cancel
              </Button>
              <Button
                type="submit"
                form="provision-user-form"
                disabled={provisionMutation.isPending}
              >
                {provisionMutation.isPending ? "Provisioning…" : "Provision"}
              </Button>
            </>
          ) : (
            <Button type="button" onClick={closeAndClear}>
              I&apos;ve saved this password — close
            </Button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

// See `apps-screen.tsx`'s own `AppDetailDrawer` doc for why `userId`/`open`
// are separate and this component is always mounted, never conditionally:
// `vaul`'s close transition needs the drawer still in the DOM for at least
// one frame after `open` flips `false`.
function UserDetailDrawer({
  userId,
  open,
  roles,
  onClose,
}: {
  userId: string | null;
  open: boolean;
  roles: RoleRecord[];
  onClose: () => void;
}) {
  const utils = trpc.useUtils();
  const [deleteConfirmOpen, setDeleteConfirmOpen] = useState(false);

  const detailQuery = trpc.users.get.useQuery({ id: userId ?? "" }, { enabled: userId !== null });
  const form = useForm<UserEditValues>({
    resolver: zodResolver(userEditSchema),
    defaultValues: { displayName: "", roleKey: "", active: true },
  });

  useEffect(() => {
    const d = detailQuery.data?.data;
    if (d === undefined) return;
    form.reset({ displayName: d.displayName, roleKey: d.roleKey, active: d.active });
    // `form.reset` is a stable reference (react-hook-form memoizes its
    // returned methods) — listed here to satisfy the lint without ever
    // causing an extra reset.
  }, [detailQuery.data, form.reset]);

  const updateMutation = trpc.users.update.useMutation({
    onSuccess: () => {
      toast({ title: "User saved", variant: "success" });
      void utils.users.list.invalidate();
      if (userId !== null) void utils.users.get.invalidate({ id: userId });
    },
  });

  const deleteMutation = trpc.users.delete.useMutation({
    onSuccess: () => {
      toast({ title: "User deleted", variant: "success" });
      setDeleteConfirmOpen(false);
      void utils.users.list.invalidate();
      onClose();
    },
  });

  function onSubmit(values: UserEditValues) {
    const etag = detailQuery.data?.etag;
    if (etag === undefined || userId === null) return;
    updateMutation.mutate({ id: userId, etag, ...values });
  }

  const isStale = updateMutation.error?.data?.code === "CONFLICT";
  const generalError = updateMutation.isError && !isStale ? updateMutation.error.message : null;

  return (
    <MoreDetailDrawer
      open={open}
      onOpenChange={(next) => !next && onClose()}
      title={detailQuery.data?.data?.email ?? "User"}
      description={userId !== null && <IdDisplay value={userId} variant="full" />}
      footer={
        <>
          <Button
            type="button"
            variant="destructive"
            size="sm"
            className="mr-auto"
            onClick={() => setDeleteConfirmOpen(true)}
          >
            Delete user
          </Button>
          <Button type="button" variant="ghost" onClick={onClose}>
            Close
          </Button>
          <Button
            type="submit"
            form="user-edit-form"
            disabled={updateMutation.isPending || detailQuery.data === undefined}
          >
            {updateMutation.isPending ? "Saving…" : "Save"}
          </Button>
        </>
      }
    >
      {detailQuery.isLoading && <Skeleton className="h-32 w-full" />}
      {detailQuery.isError && (
        <ErrorBanner>Could not read this user: {detailQuery.error.message}</ErrorBanner>
      )}

      {detailQuery.data?.data !== undefined && (
        <form
          id="user-edit-form"
          onSubmit={form.handleSubmit(onSubmit)}
          className="flex flex-col gap-4"
        >
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="user-edit-name">Display name</Label>
            <Input
              id="user-edit-name"
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
            <Label htmlFor="user-edit-role">Role</Label>
            <Controller
              control={form.control}
              name="roleKey"
              render={({ field }) => (
                <Select value={field.value} onValueChange={field.onChange}>
                  <SelectTrigger id="user-edit-role">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {roles.map((role) => (
                      <SelectItem key={role.key} value={role.key}>
                        {role.label} ({role.key})
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              )}
            />
          </div>
          <label className="flex items-center gap-2 text-caption text-foreground">
            <input type="checkbox" className="checkbox checkbox-sm" {...form.register("active")} />
            Active
          </label>

          {isStale && <StaleWriteBanner onReload={() => void detailQuery.refetch()} />}
          {generalError != null && <ErrorBanner>Save failed: {generalError}</ErrorBanner>}
        </form>
      )}

      {/* Inline, not a `Dialog` — see `apps-screen.tsx`'s
       * `ProvisionClientPanel` doc for why: this drawer is already open,
       * and a second, independently-portaled focus trap nested inside it
       * was verified live to self-dismiss the whole drawer within about
       * half a second, with no further interaction. */}
      {deleteConfirmOpen && userId !== null && (
        <div className="mt-4 flex flex-col gap-3 rounded-sm border border-state-danger-border bg-state-danger-bg p-4">
          <div>
            <p className="font-medium text-body text-state-danger-fg">Delete this user?</p>
            <p className="mt-1 text-caption text-state-danger-fg">
              owner-only, soft-deletes the row.
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
              onClick={() => deleteMutation.mutate({ id: userId })}
            >
              {deleteMutation.isPending ? "Deleting…" : "Delete"}
            </Button>
          </div>
        </div>
      )}
    </MoreDetailDrawer>
  );
}

function UsersTab({
  panelId,
  onOpenPanel,
  onClosePanel,
}: {
  panelId: string | null;
  onOpenPanel: (id: string) => void;
  onClosePanel: () => void;
}) {
  const listQuery = trpc.users.list.useQuery();
  const rolesQuery = trpc.roles.list.useQuery();
  const [provisionOpen, setProvisionOpen] = useState(false);
  // See `apps-screen.tsx`'s own `stickyPanelId` doc — `UserDetailDrawer`
  // stays mounted so its `vaul` close transition can play.
  const [stickyPanelId, setStickyPanelId] = useState<string | null>(null);
  useEffect(() => {
    if (panelId !== null) setStickyPanelId(panelId);
  }, [panelId]);

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="rounded-sm border border-edge bg-surface-2 px-3 py-2 text-caption text-muted-foreground">
          Provisioning and editing both require{" "}
          <span className="font-mono text-foreground">user:manage</span> (owner, admin) — a role
          without it gets a real error here, not a silent no-op.
        </div>
        <Button type="button" onClick={() => setProvisionOpen(true)}>
          Provision user
        </Button>
      </div>

      {listQuery.isError && (
        <ErrorBanner>Could not read users: {listQuery.error.message}</ErrorBanner>
      )}

      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>Active</TableHead>
            <TableHead>Email</TableHead>
            <TableHead className="hidden sm:table-cell">Display name</TableHead>
            <TableHead>Role</TableHead>
            <TableHead align="end" className="hidden md:table-cell">
              Last login
            </TableHead>
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
                <InlineEmptyState message="No users provisioned yet." />
              </TableCell>
            </TableRow>
          )}
          {listQuery.data?.map((user: UserListItem) => (
            <TableRow key={user.id} className="cursor-pointer" onClick={() => onOpenPanel(user.id)}>
              <TableCell>
                {user.active ? (
                  <span className="text-state-success-fg">yes</span>
                ) : (
                  <span className="text-muted-foreground">no</span>
                )}
              </TableCell>
              <TableCell>{user.email}</TableCell>
              <TableCell className="hidden sm:table-cell">{user.displayName}</TableCell>
              <TableCell mono>{user.roleKey}</TableCell>
              <TableCell align="end" className="hidden md:table-cell">
                {user.lastLoginAt !== undefined ? (
                  <TimestampDisplay value={user.lastLoginAt} />
                ) : (
                  <span className="text-muted-foreground">never</span>
                )}
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>

      <ProvisionUserDialog
        open={provisionOpen}
        onOpenChange={setProvisionOpen}
        roles={rolesQuery.data ?? []}
      />

      <UserDetailDrawer
        userId={stickyPanelId}
        open={panelId !== null}
        roles={rolesQuery.data ?? []}
        onClose={onClosePanel}
      />
    </div>
  );
}

// See `apps-screen.tsx`'s own `AppDetailDrawer` doc for why `roleId`/`open`
// are separate and this component is always mounted.
function RoleDetailDrawer({
  roleId,
  open,
  onClose,
}: {
  roleId: string | null;
  open: boolean;
  onClose: () => void;
}) {
  const utils = trpc.useUtils();
  const [deleteConfirmOpen, setDeleteConfirmOpen] = useState(false);

  const detailQuery = trpc.roles.get.useQuery({ id: roleId ?? "" }, { enabled: roleId !== null });
  const form = useForm<RoleEditValues>({
    resolver: zodResolver(roleEditSchema),
    defaultValues: { label: "", permissions: "" },
  });

  useEffect(() => {
    const d = detailQuery.data?.data;
    if (d === undefined) return;
    form.reset({ label: d.label, permissions: d.permissions.trim() });
    // `form.reset` is a stable reference (react-hook-form memoizes its
    // returned methods) — listed here to satisfy the lint without ever
    // causing an extra reset.
  }, [detailQuery.data, form.reset]);

  const updateMutation = trpc.roles.update.useMutation({
    onSuccess: () => {
      toast({ title: "Role saved", variant: "success" });
      void utils.roles.list.invalidate();
      if (roleId !== null) void utils.roles.get.invalidate({ id: roleId });
    },
  });

  const deleteMutation = trpc.roles.delete.useMutation({
    onSuccess: () => {
      toast({ title: "Role deleted", variant: "success" });
      setDeleteConfirmOpen(false);
      void utils.roles.list.invalidate();
      onClose();
    },
  });

  function onSubmit(values: RoleEditValues) {
    const etag = detailQuery.data?.etag;
    if (etag === undefined || roleId === null) return;
    updateMutation.mutate({
      id: roleId,
      etag,
      label: values.label,
      permissions: values.permissions.split(/\s+/).filter((p) => p.length > 0),
    });
  }

  const isStale = updateMutation.error?.data?.code === "CONFLICT";
  const generalError = updateMutation.isError && !isStale ? updateMutation.error.message : null;
  const builtin = detailQuery.data?.data?.builtin ?? false;

  return (
    <MoreDetailDrawer
      open={open}
      onOpenChange={(next) => !next && onClose()}
      title={detailQuery.data?.data?.label ?? "Role"}
      description={
        detailQuery.data?.data !== undefined && (
          <span className="font-mono">{detailQuery.data.data.key}</span>
        )
      }
      footer={
        <>
          {!builtin && (
            <Button
              type="button"
              variant="destructive"
              size="sm"
              className="mr-auto"
              onClick={() => setDeleteConfirmOpen(true)}
            >
              Delete role
            </Button>
          )}
          {builtin && (
            <span className="mr-auto self-center text-caption text-subtle-foreground">
              Built-in role — cannot be deleted.
            </span>
          )}
          <Button type="button" variant="ghost" onClick={onClose}>
            Close
          </Button>
          <Button
            type="submit"
            form="role-edit-form"
            disabled={updateMutation.isPending || detailQuery.data === undefined}
          >
            {updateMutation.isPending ? "Saving…" : "Save"}
          </Button>
        </>
      }
    >
      {detailQuery.isLoading && <Skeleton className="h-32 w-full" />}
      {detailQuery.isError && (
        <ErrorBanner>Could not read this role: {detailQuery.error.message}</ErrorBanner>
      )}

      {detailQuery.data?.data !== undefined && (
        <form
          id="role-edit-form"
          onSubmit={form.handleSubmit(onSubmit)}
          className="flex flex-col gap-4"
        >
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="role-edit-label">Label</Label>
            <Input
              id="role-edit-label"
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
            <Label htmlFor="role-edit-permissions">Permissions (space-separated)</Label>
            <Textarea
              id="role-edit-permissions"
              rows={3}
              className="font-mono text-caption"
              {...form.register("permissions")}
            />
            <p className="text-caption text-subtle-foreground">
              Known literals: {KNOWN_PERMISSIONS.join(", ")}
            </p>
          </div>

          {isStale && <StaleWriteBanner onReload={() => void detailQuery.refetch()} />}
          {generalError != null && <ErrorBanner>Save failed: {generalError}</ErrorBanner>}
        </form>
      )}

      {/* Inline, not a `Dialog` — see `apps-screen.tsx`'s
       * `ProvisionClientPanel` doc for why: this drawer is already open,
       * and a second, independently-portaled focus trap nested inside it
       * was verified live to self-dismiss the whole drawer within about
       * half a second, with no further interaction. */}
      {deleteConfirmOpen && roleId !== null && (
        <div className="mt-4 flex flex-col gap-3 rounded-sm border border-state-danger-border bg-state-danger-bg p-4">
          <div>
            <p className="font-medium text-body text-state-danger-fg">Delete this role?</p>
            <p className="mt-1 text-caption text-state-danger-fg">
              Fails with a foreign-key error if any user still carries it — reassign them first.
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
              onClick={() => deleteMutation.mutate({ id: roleId })}
            >
              {deleteMutation.isPending ? "Deleting…" : "Delete"}
            </Button>
          </div>
        </div>
      )}
    </MoreDetailDrawer>
  );
}

function CreateRoleDialog({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const utils = trpc.useUtils();
  const form = useForm<RoleCreateValues>({
    resolver: zodResolver(roleCreateSchema),
    defaultValues: { key: "", label: "", permissions: "" },
  });
  const createMutation = trpc.roles.create.useMutation({
    onSuccess: () => {
      toast({ title: "Role created", variant: "success" });
      form.reset();
      onOpenChange(false);
      void utils.roles.list.invalidate();
    },
    onError: (error) => {
      const fieldErrors = error.data?.fieldErrors;
      if (fieldErrors == null) return;
      for (const [field, messages] of Object.entries(fieldErrors)) {
        if (field === "key" || field === "label" || field === "permissions") {
          const msg = messages[0];
          if (msg != null) form.setError(field, { type: "server", message: msg });
        }
      }
    },
  });

  function onSubmit(values: RoleCreateValues) {
    createMutation.mutate({
      key: values.key,
      label: values.label,
      permissions: values.permissions.split(/\s+/).filter((p) => p.length > 0),
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
      <DialogContent className="max-w-[520px]">
        <DialogHeader>
          <DialogTitle>New role</DialogTitle>
        </DialogHeader>
        <form
          id="create-role-form"
          onSubmit={form.handleSubmit(onSubmit)}
          className="flex flex-col gap-4"
        >
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="role-key">Key</Label>
            <Input
              id="role-key"
              placeholder="lowercase_with_underscores"
              aria-invalid={form.formState.errors.key != null}
              {...form.register("key")}
            />
            {form.formState.errors.key != null && (
              <p className="text-caption text-state-danger-fg">
                {form.formState.errors.key.message}
              </p>
            )}
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="role-label">Label</Label>
            <Input
              id="role-label"
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
            <Label htmlFor="role-permissions">Permissions (space-separated)</Label>
            <Textarea
              id="role-permissions"
              rows={3}
              className="font-mono text-caption"
              {...form.register("permissions")}
            />
            <p className="text-caption text-subtle-foreground">
              Known literals: {KNOWN_PERMISSIONS.join(", ")}
            </p>
          </div>
          {generalError != null && <ErrorBanner>{generalError}</ErrorBanner>}
        </form>
        <DialogFooter>
          <Button type="button" variant="ghost" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button type="submit" form="create-role-form" disabled={createMutation.isPending}>
            {createMutation.isPending ? "Creating…" : "Create"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function RolesTab({
  panelId,
  onOpenPanel,
  onClosePanel,
}: {
  panelId: string | null;
  onOpenPanel: (id: string) => void;
  onClosePanel: () => void;
}) {
  const listQuery = trpc.roles.list.useQuery();
  const [createOpen, setCreateOpen] = useState(false);
  // See `apps-screen.tsx`'s own `stickyPanelId` doc — `RoleDetailDrawer`
  // stays mounted so its `vaul` close transition can play.
  const [stickyPanelId, setStickyPanelId] = useState<string | null>(null);
  useEffect(() => {
    if (panelId !== null) setStickyPanelId(panelId);
  }, [panelId]);

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="rounded-sm border border-edge bg-surface-2 px-3 py-2 text-caption text-muted-foreground">
          Creating, editing, and deleting roles all require{" "}
          <span className="font-mono text-foreground">owner</span> — the narrowest write action in
          this console.
        </div>
        <Button type="button" onClick={() => setCreateOpen(true)}>
          New role
        </Button>
      </div>

      {listQuery.isError && (
        <ErrorBanner>Could not read roles: {listQuery.error.message}</ErrorBanner>
      )}

      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>Key</TableHead>
            <TableHead>Label</TableHead>
            <TableHead className="hidden sm:table-cell">Built-in</TableHead>
            <TableHead className="hidden md:table-cell">Permissions</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {listQuery.isLoading && (
            <TableRow>
              <TableCell colSpan={4}>
                <Skeleton className="h-4 w-full" />
              </TableCell>
            </TableRow>
          )}
          {!listQuery.isLoading && (listQuery.data?.length ?? 0) === 0 && (
            <TableRow>
              <TableCell colSpan={4}>
                <InlineEmptyState message="No roles yet." />
              </TableCell>
            </TableRow>
          )}
          {listQuery.data?.map((role: RoleRecord) => (
            <TableRow key={role.id} className="cursor-pointer" onClick={() => onOpenPanel(role.id)}>
              <TableCell mono>{role.key}</TableCell>
              <TableCell>{role.label}</TableCell>
              <TableCell className="hidden sm:table-cell">
                {role.builtin ? <Badge variant="outline">built-in</Badge> : "no"}
              </TableCell>
              <TableCell className="hidden max-w-[420px] truncate text-caption md:table-cell">
                {role.permissions.trim()}
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>

      <CreateRoleDialog open={createOpen} onOpenChange={setCreateOpen} />

      <RoleDetailDrawer roleId={stickyPanelId} open={panelId !== null} onClose={onClosePanel} />
    </div>
  );
}

const TABS = ["users", "roles"] as const;

export function UsersScreen() {
  // The two tabs share one shallow route: `?tab=` picks which table is
  // showing (so a deep link or a refresh lands on the right one, not
  // always `users`), `?panel=` owns whichever row's `MoreDetailDrawer` is
  // open (D14) — scoped implicitly by the active tab, since Headless UI's
  // `TabGroup` unmounts the inactive panel by default (see this file's own
  // module doc for why that makes a single shared key safe).
  const [tab, setTab] = useQueryState(
    "tab",
    parseAsStringEnum<(typeof TABS)[number]>([...TABS]).withDefault("users"),
  );
  const [panelId, setPanelId] = useQueryState("panel", { history: "replace" });

  function openPanel(id: string) {
    void setPanelId(id);
  }
  function closePanel() {
    void setPanelId(null);
  }

  return (
    <div className="flex flex-col gap-6">
      <div className="border-edge border-b pb-6">
        <h1 className="font-medium text-foreground text-title">Users &amp; roles</h1>
        <p className="mt-1 max-w-xl text-body text-muted-foreground">
          Console accounts and the permission sets their roles carry.
        </p>
      </div>

      <Tabs
        value={tab}
        onValueChange={(next) => {
          void setTab(next === "users" ? null : (next as (typeof TABS)[number]));
          void setPanelId(null);
        }}
      >
        <TabsList>
          <TabsTrigger value="users">Users</TabsTrigger>
          <TabsTrigger value="roles">Roles</TabsTrigger>
        </TabsList>
        <TabsContent value="users">
          <UsersTab
            panelId={tab === "users" ? panelId : null}
            onOpenPanel={openPanel}
            onClosePanel={closePanel}
          />
        </TabsContent>
        <TabsContent value="roles">
          <RolesTab
            panelId={tab === "roles" ? panelId : null}
            onOpenPanel={openPanel}
            onClosePanel={closePanel}
          />
        </TabsContent>
      </Tabs>
    </div>
  );
}
