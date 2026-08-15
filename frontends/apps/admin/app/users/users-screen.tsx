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
// `isReservedRoleKey`/`isValidRoleKeyShape` (`./role-forms.ts`, a
// client-safe copy — see that module's own doc for why) block
// `system`/`app` in the create form before a request is ever sent — the
// database's own `roles_key_not_reserved_check` and
// `sms_api::auth::RESERVED_ROLE_KEYS` remain the real guards regardless,
// this is only the friendly half — enforced as a `zod` `.refine()` on the
// create form's own schema rather than a disabled-submit-button check
// computed separately.
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
//
// # R6 — layer split
//
// All markup and classes now live in `./components/*`. This file keeps
// the smart orchestration functions it already had (`ProvisionUserDialog`,
// `UserDetailDrawer`, `UsersTab`, `RoleDetailDrawer`, `CreateRoleDialog`,
// `RolesTab`, `UsersScreen`) — each does data fetching, mutations and
// derived values only, and renders exactly one dumb view. Zod schemas and
// the reserved-role-key guard moved to `./role-forms.ts`/`./user-forms.ts`
// (both pure, `role-forms.ts` carries a test file per R6's "extracted pure
// modules carry tests"). See `./role-forms.ts`'s own module doc for the
// `RESERVED_ROLE_KEYS`/`ROLE_KEY_PATTERN` carve-out reasoning this route
// was specifically asked to reason about, and for a real, reported (not
// fixed here) drift between `KNOWN_PERMISSIONS` and the server's actual
// enforced permission vocabulary.

import { zodResolver } from "@hookform/resolvers/zod";
import { trpc } from "@vsms/hooks";
import { InlineConfirm, ScreenHeader, ScreenStack, toast } from "@vsms/ui";
import { parseAsStringEnum, useQueryState } from "nuqs";
import { useEffect, useState } from "react";
import { useForm } from "react-hook-form";
import { CreateRoleDialogView } from "./components/create-role-dialog-view";
import {
  type ProvisionedUser,
  ProvisionUserDialogView,
} from "./components/provision-user-dialog-view";
import { RoleDetailDrawerView } from "./components/role-detail-drawer-view";
import { RolesTabView } from "./components/roles-tab-view";
import { UserDetailDrawerView } from "./components/user-detail-drawer-view";
import { UsersTabView } from "./components/users-tab-view";
import { UsersTabs } from "./components/users-tabs";
import {
  type RoleCreateValues,
  type RoleEditValues,
  roleCreateSchema,
  roleEditSchema,
} from "./role-forms";
import { type RoleRecord, TABS, type UserListItem, type UsersRolesTab } from "./types";
import {
  type ProvisionUserValues,
  provisionUserSchema,
  type UserEditValues,
  userEditSchema,
} from "./user-forms";

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

  function onSubmit(values: ProvisionUserValues) {
    provisionMutation.mutate(values);
  }

  const result: ProvisionedUser | undefined = provisionMutation.data;

  return (
    <ProvisionUserDialogView
      open={open}
      roles={roles}
      form={form}
      onSubmit={onSubmit}
      isPending={provisionMutation.isPending}
      isError={provisionMutation.isError}
      errorMessage={provisionMutation.error?.message ?? ""}
      result={result}
      onDone={closeAndClear}
    />
  );
}

// See `apps-screen.tsx`'s own `AppDetailDrawer` doc for why `userId`/`open`
// are separate and this component is always mounted, never conditionally.
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
  const detail = detailQuery.data?.data;

  return (
    <UserDetailDrawerView
      userId={userId}
      open={open}
      onOpenChange={(next) => !next && onClose()}
      title={detail?.email ?? "User"}
      isLoading={detailQuery.isLoading}
      loadError={detailQuery.isError ? detailQuery.error.message : null}
      hasDetail={detail !== undefined}
      form={form}
      roles={roles}
      onSubmit={onSubmit}
      isStale={isStale}
      onReload={() => void detailQuery.refetch()}
      generalError={generalError}
      isSaving={updateMutation.isPending}
      onDeleteClick={() => setDeleteConfirmOpen(true)}
      onClose={onClose}
      deleteConfirm={
        deleteConfirmOpen &&
        userId !== null && (
          <InlineConfirm
            title="Delete this user?"
            description="owner-only, soft-deletes the row."
            confirmLabel="Delete"
            pendingLabel="Deleting…"
            pending={deleteMutation.isPending}
            error={
              deleteMutation.error != null
                ? `Delete failed: ${deleteMutation.error.message}`
                : undefined
            }
            onCancel={() => setDeleteConfirmOpen(false)}
            onConfirm={() =>
              // Known-version fast path: `detailQuery.data?.etag` is the
              // same captured `ETag` `onSubmit`'s own `updateMutation`
              // call uses above — so `deleteResource` sends it directly
              // as `If-Match` with no extra `GET` round trip.
              deleteMutation.mutate({ id: userId, etag: detailQuery.data?.etag })
            }
          />
        )
      }
    />
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
    <UsersTabView
      users={listQuery.data ?? []}
      isLoading={listQuery.isLoading}
      errorMessage={listQuery.isError ? listQuery.error.message : null}
      onProvisionClick={() => setProvisionOpen(true)}
      onRowClick={(user: UserListItem) => onOpenPanel(user.id)}
    >
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
    </UsersTabView>
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
  const detail = detailQuery.data?.data;

  return (
    <RoleDetailDrawerView
      open={open}
      onOpenChange={(next) => !next && onClose()}
      title={detail?.label ?? "Role"}
      roleKey={detail?.key ?? null}
      isLoading={detailQuery.isLoading}
      loadError={detailQuery.isError ? detailQuery.error.message : null}
      hasDetail={detail !== undefined}
      builtin={detail?.builtin ?? false}
      form={form}
      onSubmit={onSubmit}
      isStale={isStale}
      onReload={() => void detailQuery.refetch()}
      generalError={generalError}
      isSaving={updateMutation.isPending}
      onDeleteClick={() => setDeleteConfirmOpen(true)}
      onClose={onClose}
      deleteConfirm={
        deleteConfirmOpen &&
        roleId !== null && (
          <InlineConfirm
            title="Delete this role?"
            description="Fails with a foreign-key error if any user still carries it — reassign them first."
            confirmLabel="Delete"
            pendingLabel="Deleting…"
            pending={deleteMutation.isPending}
            error={
              deleteMutation.error != null
                ? `Delete failed: ${deleteMutation.error.message}`
                : undefined
            }
            onCancel={() => setDeleteConfirmOpen(false)}
            onConfirm={() =>
              // Known-version fast path: `detailQuery.data?.etag` is the
              // same captured `ETag` `onSubmit`'s own `updateMutation`
              // call uses above — so `deleteResource` sends it directly
              // as `If-Match` with no extra `GET` round trip.
              deleteMutation.mutate({ id: roleId, etag: detailQuery.data?.etag })
            }
          />
        )
      }
    />
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
    <CreateRoleDialogView
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
    <RolesTabView
      roles={listQuery.data ?? []}
      isLoading={listQuery.isLoading}
      errorMessage={listQuery.isError ? listQuery.error.message : null}
      onCreateClick={() => setCreateOpen(true)}
      onRowClick={(role: RoleRecord) => onOpenPanel(role.id)}
    >
      <CreateRoleDialog open={createOpen} onOpenChange={setCreateOpen} />

      <RoleDetailDrawer roleId={stickyPanelId} open={panelId !== null} onClose={onClosePanel} />
    </RolesTabView>
  );
}

export function UsersScreen() {
  // The two tabs share one shallow route: `?tab=` picks which table is
  // showing (so a deep link or a refresh lands on the right one, not
  // always `users`), `?panel=` owns whichever row's `MoreDetailDrawer` is
  // open (D14) — scoped implicitly by the active tab, since Headless UI's
  // `TabGroup` unmounts the inactive panel by default (see this file's own
  // module doc for why that makes a single shared key safe).
  const [tab, setTab] = useQueryState(
    "tab",
    parseAsStringEnum<UsersRolesTab>([...TABS]).withDefault("users"),
  );
  const [panelId, setPanelId] = useQueryState("panel", { history: "replace" });

  function openPanel(id: string) {
    void setPanelId(id);
  }
  function closePanel() {
    void setPanelId(null);
  }

  return (
    <ScreenStack>
      <ScreenHeader
        title="Users & roles"
        description="Console accounts and the permission sets their roles carry."
      />

      <UsersTabs
        tab={tab}
        onTabChange={(next) => {
          void setTab(next === "users" ? null : next);
          void setPanelId(null);
        }}
        usersPanel={
          <UsersTab
            panelId={tab === "users" ? panelId : null}
            onOpenPanel={openPanel}
            onClosePanel={closePanel}
          />
        }
        rolesPanel={
          <RolesTab
            panelId={tab === "roles" ? panelId : null}
            onOpenPanel={openPanel}
            onClosePanel={closePanel}
          />
        }
      />
    </ScreenStack>
  );
}
