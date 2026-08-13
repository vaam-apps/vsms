"use client";

// The Users & Roles screen (#58): console accounts, their roles, and the
// permission sets those roles carry. Two tabs over one screen — assigning
// a role to a user needs the role list right there, and #58 groups them as
// one story.
//
// # Provisioning shows a password exactly once, the same discipline
// # `apps-screen.tsx` already established for a client's private key
//
// `provisionUser`'s response is read straight from the mutation hook's own
// `data` (component-local state, not the shared query cache), never
// written to storage, never routed through a toast, and
// `provisionMutation.reset()` on close clears it from memory rather than
// leaving it retrievable. See `apps-screen.tsx`'s own module doc — the
// mechanism is identical, applied to a password instead of a PEM.
//
// # No password rotation
//
// There is no "reset this user's password" action anywhere on this screen
// — no such procedure exists. `crates/sms-api/src/procedures.rs` has no
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
// (`crates/sms-api/src/auth.rs`'s own doc), this is only the friendly
// half.

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
  Tabs,
  TabsContent,
  TabsList,
  TabsTrigger,
  Textarea,
  TimestampDisplay,
  toast,
} from "@vsms/ui";
import { useEffect, useState } from "react";
import { ConsoleNav } from "../console-nav";

// `@vsms/gateway`'s own `isReservedRoleKey`/`isValidRoleKeyShape` live in
// `roles.ts`, which is `import "server-only"` at the top of the file —
// re-exporting them through `@vsms/gateway`'s index does not strip that
// marker, so importing them here (a `"use client"` component) would fail
// the build. These two are trivial, pure regex checks with no security
// weight of their own (the database `CHECK` and `RESERVED_ROLE_KEYS` in
// `crates/sms-api/src/auth.rs` are the real guards regardless — see this
// file's own module doc), so they're duplicated locally rather than
// pulling a new shared, client-safe package into existence for two
// one-line functions.
const RESERVED_ROLE_KEYS = new Set(["system", "app"]);
const ROLE_KEY_PATTERN = /^[a-z][a-z0-9_]{2,31}$/;
function isReservedRoleKey(key: string): boolean {
  return RESERVED_ROLE_KEYS.has(key);
}
function isValidRoleKeyShape(key: string): boolean {
  return ROLE_KEY_PATTERN.test(key);
}

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
  const [email, setEmail] = useState("");
  const [displayName, setDisplayName] = useState("");
  const [roleKey, setRoleKey] = useState(roles[0]?.key ?? "");
  const provisionMutation = trpc.users.provision.useMutation({
    onSuccess: () => {
      void utils.users.list.invalidate();
    },
  });

  function closeAndClear() {
    provisionMutation.reset();
    setEmail("");
    setDisplayName("");
    onOpenChange(false);
  }

  const result = provisionMutation.data;

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
          <div className="flex flex-col gap-4">
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="user-email">Email</Label>
              <Input
                id="user-email"
                type="email"
                value={email}
                onChange={(e) => setEmail(e.target.value)}
              />
            </div>
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="user-display-name">Display name</Label>
              <Input
                id="user-display-name"
                value={displayName}
                onChange={(e) => setDisplayName(e.target.value)}
              />
            </div>
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="user-role">Role</Label>
              <Select value={roleKey} onValueChange={setRoleKey}>
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
            </div>
            {provisionMutation.isError && (
              <div className="rounded-sm border border-state-danger-border bg-state-danger-bg px-3 py-2 text-caption text-state-danger-fg">
                {provisionMutation.error.message}
              </div>
            )}
          </div>
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
              <Button type="button" variant="ghost" onClick={() => onOpenChange(false)}>
                Cancel
              </Button>
              <Button
                type="button"
                disabled={
                  email.trim().length === 0 ||
                  displayName.trim().length === 0 ||
                  roleKey.length === 0 ||
                  provisionMutation.isPending
                }
                onClick={() =>
                  provisionMutation.mutate({
                    email: email.trim(),
                    displayName: displayName.trim(),
                    roleKey,
                  })
                }
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

function UsersTab() {
  const listQuery = trpc.users.list.useQuery();
  const rolesQuery = trpc.roles.list.useQuery();
  const utils = trpc.useUtils();
  const [provisionOpen, setProvisionOpen] = useState(false);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [deleteConfirmId, setDeleteConfirmId] = useState<string | null>(null);

  const detailQuery = trpc.users.get.useQuery(
    { id: selectedId ?? "" },
    { enabled: selectedId !== null },
  );
  const [form, setForm] = useState<{
    displayName: string;
    roleKey: string;
    active: boolean;
  } | null>(null);

  useEffect(() => {
    if (detailQuery.data?.data !== undefined) {
      const d = detailQuery.data.data;
      setForm({ displayName: d.displayName, roleKey: d.roleKey, active: d.active });
    }
  }, [detailQuery.data]);

  const updateMutation = trpc.users.update.useMutation({
    onSuccess: () => {
      toast({ title: "User saved", variant: "success" });
      void utils.users.list.invalidate();
      void utils.users.get.invalidate({ id: selectedId ?? "" });
    },
  });

  const deleteMutation = trpc.users.delete.useMutation({
    onSuccess: () => {
      toast({ title: "User deleted", variant: "success" });
      setSelectedId(null);
      setDeleteConfirmId(null);
      void utils.users.list.invalidate();
    },
  });

  function closeDetail() {
    setSelectedId(null);
    setForm(null);
    updateMutation.reset();
  }

  return (
    <div className="flex flex-col gap-4">
      <div className="flex items-center justify-between">
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
        <div className="rounded-sm border border-state-danger-border bg-state-danger-bg px-3 py-2 text-caption text-state-danger-fg">
          Could not read users: {listQuery.error.message}
        </div>
      )}

      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>Active</TableHead>
            <TableHead>Email</TableHead>
            <TableHead>Display name</TableHead>
            <TableHead>Role</TableHead>
            <TableHead align="end">Last login</TableHead>
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
            <tr>
              <td colSpan={5}>
                <InlineEmptyState message="No users provisioned yet." />
              </td>
            </tr>
          )}
          {listQuery.data?.map((user: UserListItem) => (
            <TableRow
              key={user.id}
              className="cursor-pointer"
              onClick={() => setSelectedId(user.id)}
            >
              <TableCell>
                {user.active ? (
                  <span className="text-state-success-fg">yes</span>
                ) : (
                  <span className="text-muted-foreground">no</span>
                )}
              </TableCell>
              <TableCell>{user.email}</TableCell>
              <TableCell>{user.displayName}</TableCell>
              <TableCell mono>{user.roleKey}</TableCell>
              <TableCell align="end">
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

      <Dialog open={selectedId !== null} onOpenChange={(open) => !open && closeDetail()}>
        <DialogContent className="max-w-[520px]">
          <DialogHeader>
            <DialogTitle>
              {detailQuery.data?.data !== undefined ? detailQuery.data.data.email : "User"}
            </DialogTitle>
            <DialogDescription>
              {selectedId !== null && <IdDisplay value={selectedId} variant="full" />}
            </DialogDescription>
          </DialogHeader>

          {detailQuery.isLoading && <Skeleton className="h-32 w-full" />}

          {detailQuery.data?.data !== undefined && form !== null && (
            <div className="flex flex-col gap-4">
              <div className="flex flex-col gap-1.5">
                <Label htmlFor="user-edit-name">Display name</Label>
                <Input
                  id="user-edit-name"
                  value={form.displayName}
                  onChange={(e) => setForm({ ...form, displayName: e.target.value })}
                />
              </div>
              <div className="flex flex-col gap-1.5">
                <Label htmlFor="user-edit-role">Role</Label>
                <Select
                  value={form.roleKey}
                  onValueChange={(value) => setForm({ ...form, roleKey: value })}
                >
                  <SelectTrigger id="user-edit-role">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {(rolesQuery.data ?? []).map((role) => (
                      <SelectItem key={role.key} value={role.key}>
                        {role.label} ({role.key})
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
              <label className="flex items-center gap-2 text-caption text-foreground">
                <input
                  type="checkbox"
                  checked={form.active}
                  onChange={(e) => setForm({ ...form, active: e.target.checked })}
                />
                Active
              </label>

              {updateMutation.isError && (
                <div className="rounded-sm border border-state-danger-border bg-state-danger-bg px-3 py-2 text-caption text-state-danger-fg">
                  Save failed: {updateMutation.error.message}
                </div>
              )}

              <div className="flex justify-between border-edge border-t pt-4">
                <Button
                  type="button"
                  variant="destructive"
                  size="sm"
                  onClick={() => setDeleteConfirmId(selectedId)}
                >
                  Delete user
                </Button>
                <Button
                  type="button"
                  disabled={updateMutation.isPending || detailQuery.data?.etag === undefined}
                  onClick={() => {
                    const etag = detailQuery.data?.etag;
                    if (selectedId === null || etag === undefined) return;
                    updateMutation.mutate({
                      id: selectedId,
                      etag,
                      displayName: form.displayName,
                      roleKey: form.roleKey,
                      active: form.active,
                    });
                  }}
                >
                  {updateMutation.isPending ? "Saving…" : "Save"}
                </Button>
              </div>
            </div>
          )}
        </DialogContent>
      </Dialog>

      <Dialog
        open={deleteConfirmId !== null}
        onOpenChange={(open) => !open && setDeleteConfirmId(null)}
      >
        <DialogContent className="max-w-[440px]">
          <DialogHeader>
            <DialogTitle>Delete this user?</DialogTitle>
            <DialogDescription>owner-only, soft-deletes the row.</DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button type="button" variant="ghost" onClick={() => setDeleteConfirmId(null)}>
              Cancel
            </Button>
            <Button
              type="button"
              variant="destructive"
              disabled={deleteMutation.isPending}
              onClick={() =>
                deleteConfirmId !== null && deleteMutation.mutate({ id: deleteConfirmId })
              }
            >
              {deleteMutation.isPending ? "Deleting…" : "Delete"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}

function RolesTab() {
  const listQuery = trpc.roles.list.useQuery();
  const utils = trpc.useUtils();
  const [createOpen, setCreateOpen] = useState(false);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [deleteConfirmId, setDeleteConfirmId] = useState<string | null>(null);

  const [createKey, setCreateKey] = useState("");
  const [createLabel, setCreateLabel] = useState("");
  const [createPermissions, setCreatePermissions] = useState("");
  const createMutation = trpc.roles.create.useMutation({
    onSuccess: () => {
      toast({ title: "Role created", variant: "success" });
      setCreateKey("");
      setCreateLabel("");
      setCreatePermissions("");
      setCreateOpen(false);
      void utils.roles.list.invalidate();
    },
  });

  const detailQuery = trpc.roles.get.useQuery(
    { id: selectedId ?? "" },
    { enabled: selectedId !== null },
  );
  const [editForm, setEditForm] = useState<{
    label: string;
    description: string;
    permissions: string;
  } | null>(null);
  useEffect(() => {
    if (detailQuery.data?.data !== undefined) {
      const d = detailQuery.data.data;
      setEditForm({
        label: d.label,
        description: d.description ?? "",
        permissions: d.permissions.trim(),
      });
    }
  }, [detailQuery.data]);

  const updateMutation = trpc.roles.update.useMutation({
    onSuccess: () => {
      toast({ title: "Role saved", variant: "success" });
      void utils.roles.list.invalidate();
      void utils.roles.get.invalidate({ id: selectedId ?? "" });
    },
  });

  const deleteMutation = trpc.roles.delete.useMutation({
    onSuccess: () => {
      toast({ title: "Role deleted", variant: "success" });
      setSelectedId(null);
      setDeleteConfirmId(null);
      void utils.roles.list.invalidate();
    },
  });

  const createKeyReserved = isReservedRoleKey(createKey.trim());
  const createKeyShapeOk = createKey.trim().length === 0 || isValidRoleKeyShape(createKey.trim());

  return (
    <div className="flex flex-col gap-4">
      <div className="flex items-center justify-between">
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
        <div className="rounded-sm border border-state-danger-border bg-state-danger-bg px-3 py-2 text-caption text-state-danger-fg">
          Could not read roles: {listQuery.error.message}
        </div>
      )}

      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>Key</TableHead>
            <TableHead>Label</TableHead>
            <TableHead>Built-in</TableHead>
            <TableHead>Permissions</TableHead>
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
          {listQuery.data?.map((role: RoleRecord) => (
            <TableRow
              key={role.id}
              className="cursor-pointer"
              onClick={() => setSelectedId(role.id)}
            >
              <TableCell mono>{role.key}</TableCell>
              <TableCell>{role.label}</TableCell>
              <TableCell>{role.builtin ? "yes" : "no"}</TableCell>
              <TableCell className="max-w-[420px] truncate text-caption">
                {role.permissions.trim()}
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>

      <Dialog open={createOpen} onOpenChange={setCreateOpen}>
        <DialogContent className="max-w-[520px]">
          <DialogHeader>
            <DialogTitle>New role</DialogTitle>
          </DialogHeader>
          <div className="flex flex-col gap-4">
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="role-key">Key</Label>
              <Input
                id="role-key"
                placeholder="lowercase_with_underscores"
                value={createKey}
                onChange={(e) => setCreateKey(e.target.value)}
              />
              {createKeyReserved && (
                <p className="text-caption text-state-danger-fg">
                  &quot;{createKey.trim()}&quot; is reserved and can never be assigned to a role.
                </p>
              )}
              {!createKeyReserved && !createKeyShapeOk && (
                <p className="text-caption text-state-danger-fg">
                  Must start with a letter, 3-32 chars, lowercase letters/digits/underscore only.
                </p>
              )}
            </div>
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="role-label">Label</Label>
              <Input
                id="role-label"
                value={createLabel}
                onChange={(e) => setCreateLabel(e.target.value)}
              />
            </div>
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="role-permissions">Permissions (space-separated)</Label>
              <Textarea
                id="role-permissions"
                rows={3}
                className="font-mono text-caption"
                value={createPermissions}
                onChange={(e) => setCreatePermissions(e.target.value)}
              />
              <p className="text-caption text-subtle-foreground">
                Known literals: {KNOWN_PERMISSIONS.join(", ")}
              </p>
            </div>
            {createMutation.isError && (
              <div className="rounded-sm border border-state-danger-border bg-state-danger-bg px-3 py-2 text-caption text-state-danger-fg">
                {createMutation.error.message}
              </div>
            )}
          </div>
          <DialogFooter>
            <Button type="button" variant="ghost" onClick={() => setCreateOpen(false)}>
              Cancel
            </Button>
            <Button
              type="button"
              disabled={
                createKey.trim().length === 0 ||
                createLabel.trim().length === 0 ||
                createKeyReserved ||
                !createKeyShapeOk ||
                createMutation.isPending
              }
              onClick={() =>
                createMutation.mutate({
                  key: createKey.trim(),
                  label: createLabel.trim(),
                  permissions: createPermissions.split(/\s+/).filter((p) => p.length > 0),
                })
              }
            >
              {createMutation.isPending ? "Creating…" : "Create"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog open={selectedId !== null} onOpenChange={(open) => !open && setSelectedId(null)}>
        <DialogContent className="max-w-[520px]">
          <DialogHeader>
            <DialogTitle>
              {detailQuery.data?.data !== undefined ? detailQuery.data.data.label : "Role"}
            </DialogTitle>
            <DialogDescription>
              {detailQuery.data?.data !== undefined && (
                <span className="font-mono">{detailQuery.data.data.key}</span>
              )}
            </DialogDescription>
          </DialogHeader>

          {detailQuery.isLoading && <Skeleton className="h-32 w-full" />}

          {detailQuery.data?.data !== undefined && editForm !== null && (
            <div className="flex flex-col gap-4">
              <div className="flex flex-col gap-1.5">
                <Label htmlFor="role-edit-label">Label</Label>
                <Input
                  id="role-edit-label"
                  value={editForm.label}
                  onChange={(e) => setEditForm({ ...editForm, label: e.target.value })}
                />
              </div>
              <div className="flex flex-col gap-1.5">
                <Label htmlFor="role-edit-permissions">Permissions (space-separated)</Label>
                <Textarea
                  id="role-edit-permissions"
                  rows={3}
                  className="font-mono text-caption"
                  value={editForm.permissions}
                  onChange={(e) => setEditForm({ ...editForm, permissions: e.target.value })}
                />
              </div>
              {updateMutation.isError && (
                <div className="rounded-sm border border-state-danger-border bg-state-danger-bg px-3 py-2 text-caption text-state-danger-fg">
                  Save failed: {updateMutation.error.message}
                </div>
              )}
              <div className="flex justify-between border-edge border-t pt-4">
                {!detailQuery.data.data.builtin && (
                  <Button
                    type="button"
                    variant="destructive"
                    size="sm"
                    onClick={() => setDeleteConfirmId(selectedId)}
                  >
                    Delete role
                  </Button>
                )}
                {detailQuery.data.data.builtin && (
                  <span className="self-center text-caption text-subtle-foreground">
                    Built-in role — cannot be deleted.
                  </span>
                )}
                <Button
                  type="button"
                  disabled={updateMutation.isPending || detailQuery.data?.etag === undefined}
                  onClick={() => {
                    const etag = detailQuery.data?.etag;
                    if (selectedId === null || etag === undefined) return;
                    updateMutation.mutate({
                      id: selectedId,
                      etag,
                      label: editForm.label,
                      permissions: editForm.permissions.split(/\s+/).filter((p) => p.length > 0),
                    });
                  }}
                >
                  {updateMutation.isPending ? "Saving…" : "Save"}
                </Button>
              </div>
            </div>
          )}
        </DialogContent>
      </Dialog>

      <Dialog
        open={deleteConfirmId !== null}
        onOpenChange={(open) => !open && setDeleteConfirmId(null)}
      >
        <DialogContent className="max-w-[440px]">
          <DialogHeader>
            <DialogTitle>Delete this role?</DialogTitle>
            <DialogDescription>
              Fails with a foreign-key error if any user still carries it — reassign them first.
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button type="button" variant="ghost" onClick={() => setDeleteConfirmId(null)}>
              Cancel
            </Button>
            <Button
              type="button"
              variant="destructive"
              disabled={deleteMutation.isPending}
              onClick={() =>
                deleteConfirmId !== null && deleteMutation.mutate({ id: deleteConfirmId })
              }
            >
              {deleteMutation.isPending ? "Deleting…" : "Delete"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}

export function UsersScreen() {
  return (
    <main className="mx-auto flex max-w-[1200px] flex-col gap-6 px-6 py-10">
      <header className="flex items-start justify-between gap-4 border-edge border-b pb-6">
        <div>
          <p className="font-mono text-micro text-subtle-foreground tracking-[0.03em]">
            vsms admin console
          </p>
          <h1 className="mt-1 font-medium text-foreground text-title">Users &amp; roles</h1>
          <p className="mt-1 max-w-xl text-body text-muted-foreground">
            Console accounts and the permission sets their roles carry.
          </p>
        </div>
        <ConsoleNav current="/users" />
      </header>

      <Tabs defaultValue="users">
        <TabsList>
          <TabsTrigger value="users">Users</TabsTrigger>
          <TabsTrigger value="roles">Roles</TabsTrigger>
        </TabsList>
        <TabsContent value="users">
          <UsersTab />
        </TabsContent>
        <TabsContent value="roles">
          <RolesTab />
        </TabsContent>
      </Tabs>
    </main>
  );
}
