"use client";

// The Sender IDs screen (#53): sender ID CRUD plus per-provider registration
// status, with rejection reasons front and centre. "Sender IDs need
// registration per provider, and the status is per (sender ID, provider)
// pair rather than global. Surface rejection reasons — they are
// actionable" — the issue's own words.
//
// # Why one page, no dialog-inside-a-dialog
//
// `SenderId` and `SenderIdRegistration` are two models joined client-side
// (`@vsms/gateway/senders.ts`'s own module doc: neither embeds the other on
// the wire). A registration's own edit action (status/reference/rejection
// reason) needed a natural home that isn't nested inside the sender id's
// own edit dialog — Radix dialogs can stack, but a dialog whose own content
// opens a second dialog is exactly the kind of "why does this feel heavy"
// this project's design philosophy warns against. So the selected sender
// id's detail — its own editable fields *and* its registrations table — is
// inline content on the page (the same shape `message-detail-screen.tsx`
// uses for a single record), and only the smaller, single-purpose actions
// (create sender id, register with a new provider, edit one registration,
// confirm a resubmit) are dialogs, each a sibling of the page, never nested.
//
// # Registration status is a plain `String`, not a governed enum
//
// `@vsms/ui`'s own `Badge` doc: "Never use `Badge` for a message or job
// state — that's `StatusPill`'s job... mixing them is how the status
// language erodes." `SenderIdRegistration.status` genuinely isn't part of
// that governed vocabulary (`schema.cstack` declares it a bare `String`,
// and only `"approved"` is load-bearing anywhere server-side —
// `procedures.rs::resolve_sender_id`'s own `APPROVED` const). So this
// screen renders it as a `Badge` (a tag, exactly what the component is for)
// with a colour hint layered on top locally — never a fake `StatusPill` for
// a value the schema itself never closed into an enum.
//
// # Provider names, not raw ids
//
// `trpc.providers.list` already exists (#54) and needs no new server work —
// this screen joins registrations to it purely for display (`providerName`)
// and to build the "register with a new provider" picker's own options
// (every provider that doesn't already have a registration row for the
// selected sender id).

import type { inferRouterOutputs } from "@trpc/server";
import type { AppRouter } from "@vsms/api";
import { trpc } from "@vsms/hooks";
import {
  Badge,
  Button,
  Card,
  CardBody,
  CardHeader,
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
  Textarea,
  ThemeToggle,
  TimestampDisplay,
  toast,
} from "@vsms/ui";
import { useEffect, useMemo, useState } from "react";

type RouterOutputs = inferRouterOutputs<AppRouter>;
type SenderIdListItem = RouterOutputs["senderIds"]["list"][number];
type RegistrationListItem = RouterOutputs["senderIdRegistrations"]["list"][number];
type ProviderListItem = RouterOutputs["providers"]["list"][number];

const KNOWN_STATUSES = ["pending", "submitted", "approved", "rejected"] as const;

const STATUS_CLASSES: Record<string, string> = {
  approved: "text-state-success-fg border-state-success-border bg-state-success-bg",
  rejected: "text-state-danger-fg border-state-danger-border bg-state-danger-bg",
  pending: "text-muted-foreground",
  submitted: "text-muted-foreground",
};

function RegistrationStatusBadge({ status }: { status: string }) {
  const extra = STATUS_CLASSES[status] ?? "text-muted-foreground";
  return (
    <Badge variant={status in STATUS_CLASSES ? "neutral" : "outline"} className={extra}>
      {status}
    </Badge>
  );
}

interface SenderIdFormState {
  value: string;
  kind: string;
  notes: string;
  active: boolean;
}

interface RegistrationFormState {
  status: string;
  reference: string;
  rejectionReason: string;
}

export function SenderIdsScreen() {
  const utils = trpc.useUtils();
  const listQuery = trpc.senderIds.list.useQuery();
  const registrationsQuery = trpc.senderIdRegistrations.list.useQuery();
  const providersQuery = trpc.providers.list.useQuery();

  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [form, setForm] = useState<SenderIdFormState | null>(null);

  useEffect(() => {
    const selected = listQuery.data?.find((s) => s.id === selectedId);
    if (selected !== undefined) {
      setForm({
        value: selected.value,
        kind: selected.kind,
        notes: selected.notes ?? "",
        active: selected.active,
      });
    }
  }, [listQuery.data, selectedId]);

  const updateSenderMutation = trpc.senderIds.update.useMutation({
    onSuccess: () => {
      toast({ title: "Sender ID saved", variant: "success" });
      void utils.senderIds.list.invalidate();
    },
  });

  const [createOpen, setCreateOpen] = useState(false);
  const [createForm, setCreateForm] = useState({ value: "", kind: "", notes: "" });
  const createSenderMutation = trpc.senderIds.create.useMutation({
    onSuccess: (created) => {
      toast({ title: "Sender ID created", variant: "success" });
      setCreateOpen(false);
      setCreateForm({ value: "", kind: "", notes: "" });
      void utils.senderIds.list.invalidate();
      setSelectedId(created.id);
    },
  });

  const [registerProviderId, setRegisterProviderId] = useState<string | null>(null);
  const createRegistrationMutation = trpc.senderIdRegistrations.create.useMutation({
    onSuccess: () => {
      toast({ title: "Registration created", variant: "success" });
      setRegisterProviderId(null);
      void utils.senderIdRegistrations.list.invalidate();
    },
  });

  const [editingRegistration, setEditingRegistration] = useState<RegistrationListItem | null>(null);
  const [registrationForm, setRegistrationForm] = useState<RegistrationFormState | null>(null);
  const updateRegistrationMutation = trpc.senderIdRegistrations.update.useMutation({
    onSuccess: () => {
      toast({ title: "Registration saved", variant: "success" });
      setEditingRegistration(null);
      setRegistrationForm(null);
      void utils.senderIdRegistrations.invalidate();
    },
  });

  const [resubmitTarget, setResubmitTarget] = useState<RegistrationListItem | null>(null);
  const resubmitMutation = trpc.senderIdRegistrations.update.useMutation({
    onSuccess: () => {
      toast({ title: "Registration resubmitted", variant: "success" });
      setResubmitTarget(null);
      void utils.senderIdRegistrations.invalidate();
    },
  });

  const providerById = useMemo(() => {
    const map = new Map<string, ProviderListItem>();
    for (const provider of providersQuery.data ?? []) map.set(provider.id, provider);
    return map;
  }, [providersQuery.data]);

  const registrationsBySender = useMemo(() => {
    const map = new Map<string, RegistrationListItem[]>();
    for (const registration of registrationsQuery.data ?? []) {
      const list = map.get(registration.senderIdId) ?? [];
      list.push(registration);
      map.set(registration.senderIdId, list);
    }
    return map;
  }, [registrationsQuery.data]);

  const selectedRegistrations =
    selectedId != null ? (registrationsBySender.get(selectedId) ?? []) : [];
  const registeredProviderIds = new Set(selectedRegistrations.map((r) => r.providerId));
  const unregisteredProviders = (providersQuery.data ?? []).filter(
    (p) => !registeredProviderIds.has(p.id),
  );

  function summarize(senderId: string): string {
    const registrations = registrationsBySender.get(senderId) ?? [];
    if (registrations.length === 0) return "not registered anywhere";
    return registrations
      .map((r) => `${providerById.get(r.providerId)?.key ?? r.providerId}: ${r.status}`)
      .join(" · ");
  }

  function saveSender() {
    if (selectedId === null || form === null) return;
    const selected = listQuery.data?.find((s) => s.id === selectedId);
    if (selected === undefined) return;
    updateSenderMutation.mutate({
      id: selectedId,
      etag: String(selected.version),
      value: form.value,
      kind: form.kind,
      // An empty string here is a real, sent value ("clear notes"), not an
      // omitted key — see `@vsms/gateway/senders.ts`'s own module doc: a
      // verified cratestack-macros gap means `undefined`/`null` cannot
      // clear a nullable column over this REST route, only a genuine `""`.
      notes: form.notes,
      active: form.active,
    });
  }

  function openRegistrationEdit(registration: RegistrationListItem) {
    setEditingRegistration(registration);
    setRegistrationForm({
      status: registration.status,
      reference: registration.reference ?? "",
      rejectionReason: registration.rejectionReason ?? "",
    });
  }

  function saveRegistration() {
    if (editingRegistration === null || registrationForm === null) return;
    updateRegistrationMutation.mutate({
      id: editingRegistration.id,
      etag: String(editingRegistration.version),
      status: registrationForm.status,
      // Empty strings, sent as-is — see `saveSender`'s own comment above
      // and `@vsms/gateway/senders.ts`'s module doc for why this is the
      // real "clear this field" value on this route, not a placeholder for
      // "no change."
      reference: registrationForm.reference,
      rejectionReason: registrationForm.rejectionReason,
    });
  }

  function confirmResubmit() {
    if (resubmitTarget === null) return;
    resubmitMutation.mutate({
      id: resubmitTarget.id,
      etag: String(resubmitTarget.version),
      status: "pending",
      submittedAt: new Date().toISOString(),
      // `""`, not `null` — a verified no-op on this route (module doc).
      rejectionReason: "",
    });
  }

  function submitRegistration() {
    if (selectedId === null || registerProviderId === null) return;
    createRegistrationMutation.mutate({
      senderIdId: selectedId,
      providerId: registerProviderId,
      status: "pending",
    });
  }

  const selectedSenderId = listQuery.data?.find((s) => s.id === selectedId);

  return (
    <main className="mx-auto flex max-w-[1200px] flex-col gap-6 px-6 py-10">
      <header className="flex items-start justify-between gap-4 border-edge border-b pb-6">
        <div>
          <p className="font-mono text-micro text-subtle-foreground tracking-[0.03em]">
            vsms admin console
          </p>
          <h1 className="mt-1 font-medium text-foreground text-title">Sender IDs</h1>
          <p className="mt-1 max-w-xl text-body text-muted-foreground">
            Brand identifiers and their per-provider registration status — a sender ID isn't
            "approved" or "rejected" globally, only against one provider at a time.
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
            href="/webhooks"
            className="text-caption text-muted-foreground underline decoration-edge-strong underline-offset-2 hover:decoration-foreground"
          >
            Webhooks
          </a>
          <a
            href="/providers"
            className="text-caption text-muted-foreground underline decoration-edge-strong underline-offset-2 hover:decoration-foreground"
          >
            Providers
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
        Reads and writes act as you — saving here requires your own role to carry{" "}
        <span className="font-mono text-foreground">sender:manage</span> (owner, admin, and operator
        all do by default).
      </div>

      {(listQuery.isError || registrationsQuery.isError) && (
        <div className="rounded-sm border border-state-danger-border bg-state-danger-bg px-3 py-2 text-caption text-state-danger-fg">
          Could not read sender IDs: {listQuery.error?.message ?? registrationsQuery.error?.message}
        </div>
      )}

      <div className="flex items-center justify-between">
        <h2 className="font-medium text-body text-foreground">All sender IDs</h2>
        <Button type="button" size="sm" onClick={() => setCreateOpen(true)}>
          New sender ID
        </Button>
      </div>

      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>Active</TableHead>
            <TableHead>Value</TableHead>
            <TableHead>Kind</TableHead>
            <TableHead>Registrations</TableHead>
            <TableHead align="end">Updated</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {listQuery.isLoading &&
            Array.from({ length: 3 }).map((_, i) => (
              // biome-ignore lint/suspicious/noArrayIndexKey: static skeleton rows, never reordered or diffed
              <TableRow key={i}>
                <TableCell colSpan={5}>
                  <Skeleton className="h-4 w-full" />
                </TableCell>
              </TableRow>
            ))}

          {!listQuery.isLoading && (listQuery.data?.length ?? 0) === 0 && (
            <tr>
              <td colSpan={5}>
                <InlineEmptyState message="No sender IDs configured yet." />
              </td>
            </tr>
          )}

          {listQuery.data?.map((senderId: SenderIdListItem) => (
            <TableRow
              key={senderId.id}
              className="cursor-pointer"
              onClick={() => setSelectedId(senderId.id)}
            >
              <TableCell>
                {senderId.active ? (
                  <span className="text-state-success-fg">active</span>
                ) : (
                  <span className="text-muted-foreground">inactive</span>
                )}
              </TableCell>
              <TableCell mono>{senderId.value}</TableCell>
              <TableCell mono>{senderId.kind}</TableCell>
              <TableCell>
                <span className="text-caption text-muted-foreground">{summarize(senderId.id)}</span>
              </TableCell>
              <TableCell align="end">
                <TimestampDisplay value={senderId.updatedAt} />
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>

      {selectedSenderId != null && form != null && (
        <Card>
          <CardHeader
            title={selectedSenderId.value}
            meta={<IdDisplay value={selectedSenderId.id} variant="full" />}
            action={
              <Button type="button" variant="ghost" size="sm" onClick={() => setSelectedId(null)}>
                Close
              </Button>
            }
          />
          <CardBody className="flex flex-col gap-4">
            <div className="grid grid-cols-2 gap-3 sm:grid-cols-4">
              <div className="flex flex-col gap-1.5">
                <Label htmlFor="sender-value">Value</Label>
                <Input
                  id="sender-value"
                  value={form.value}
                  onChange={(e) => setForm({ ...form, value: e.target.value })}
                />
              </div>
              <div className="flex flex-col gap-1.5">
                <Label htmlFor="sender-kind">Kind</Label>
                <Input
                  id="sender-kind"
                  placeholder="e.g. alphanumeric"
                  value={form.kind}
                  onChange={(e) => setForm({ ...form, kind: e.target.value })}
                />
              </div>
              <div className="col-span-2 flex flex-col gap-1.5">
                <Label htmlFor="sender-notes">Notes</Label>
                <Input
                  id="sender-notes"
                  value={form.notes}
                  onChange={(e) => setForm({ ...form, notes: e.target.value })}
                />
              </div>
            </div>
            <label className="flex items-center gap-2 text-body text-foreground">
              <input
                type="checkbox"
                checked={form.active}
                onChange={(e) => setForm({ ...form, active: e.target.checked })}
                className="checkbox"
              />
              Active — eligible for <span className="font-mono">sendMessage</span> to resolve as a
              default or explicit sender
            </label>
            {updateSenderMutation.isError && (
              <div className="rounded-sm border border-state-danger-border bg-state-danger-bg px-3 py-2 text-caption text-state-danger-fg">
                Save failed: {updateSenderMutation.error.message}
              </div>
            )}
            <div>
              <Button
                type="button"
                size="sm"
                disabled={updateSenderMutation.isPending}
                onClick={saveSender}
              >
                {updateSenderMutation.isPending ? "Saving…" : "Save"}
              </Button>
            </div>
          </CardBody>

          <div className="border-edge border-t px-4 pt-4 pb-4">
            <div className="mb-3 flex items-center justify-between">
              <h3 className="font-medium text-body text-foreground">Registrations, per provider</h3>
              <Button
                type="button"
                variant="secondary"
                size="sm"
                disabled={unregisteredProviders.length === 0}
                onClick={() => setRegisterProviderId(unregisteredProviders[0]?.id ?? null)}
              >
                Register with a new provider
              </Button>
            </div>

            {selectedRegistrations.length === 0 ? (
              <InlineEmptyState message="Not registered with any provider yet." />
            ) : (
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>Provider</TableHead>
                    <TableHead>Status</TableHead>
                    <TableHead>Submitted</TableHead>
                    <TableHead>Approved</TableHead>
                    <TableHead>Reference</TableHead>
                    <TableHead>Rejection reason</TableHead>
                    <TableHead align="end">Action</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {selectedRegistrations.map((registration) => (
                    <TableRow key={registration.id}>
                      <TableCell mono>
                        {providerById.get(registration.providerId)?.displayName ??
                          registration.providerId}
                      </TableCell>
                      <TableCell>
                        <RegistrationStatusBadge status={registration.status} />
                      </TableCell>
                      <TableCell>
                        {registration.submittedAt != null ? (
                          <TimestampDisplay value={registration.submittedAt} />
                        ) : (
                          <span className="text-muted-foreground">—</span>
                        )}
                      </TableCell>
                      <TableCell>
                        {registration.approvedAt != null ? (
                          <TimestampDisplay value={registration.approvedAt} />
                        ) : (
                          <span className="text-muted-foreground">—</span>
                        )}
                      </TableCell>
                      <TableCell mono>{registration.reference ?? "—"}</TableCell>
                      <TableCell>
                        {registration.rejectionReason != null ? (
                          <span className="text-state-danger-fg">
                            {registration.rejectionReason}
                          </span>
                        ) : (
                          <span className="text-muted-foreground">—</span>
                        )}
                      </TableCell>
                      <TableCell align="end">
                        <div className="flex justify-end gap-2">
                          <Button
                            type="button"
                            variant="ghost"
                            size="sm"
                            onClick={() => openRegistrationEdit(registration)}
                          >
                            Edit
                          </Button>
                          {registration.status === "rejected" && (
                            <Button
                              type="button"
                              variant="secondary"
                              size="sm"
                              onClick={() => setResubmitTarget(registration)}
                            >
                              Resubmit
                            </Button>
                          )}
                        </div>
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            )}
          </div>
        </Card>
      )}

      {/* Create sender ID */}
      <Dialog open={createOpen} onOpenChange={(open) => !open && setCreateOpen(false)}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>New sender ID</DialogTitle>
            <DialogDescription>
              Created inactive — activate it from the detail panel once it's ready to be used.
            </DialogDescription>
          </DialogHeader>
          <div className="flex flex-col gap-4">
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="new-sender-value">Value (3–11 characters)</Label>
              <Input
                id="new-sender-value"
                value={createForm.value}
                onChange={(e) => setCreateForm({ ...createForm, value: e.target.value })}
              />
            </div>
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="new-sender-kind">Kind</Label>
              <Input
                id="new-sender-kind"
                placeholder="e.g. alphanumeric"
                value={createForm.kind}
                onChange={(e) => setCreateForm({ ...createForm, kind: e.target.value })}
              />
            </div>
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="new-sender-notes">Notes (optional)</Label>
              <Input
                id="new-sender-notes"
                value={createForm.notes}
                onChange={(e) => setCreateForm({ ...createForm, notes: e.target.value })}
              />
            </div>
            {createSenderMutation.isError && (
              <div className="rounded-sm border border-state-danger-border bg-state-danger-bg px-3 py-2 text-caption text-state-danger-fg">
                Create failed: {createSenderMutation.error.message}
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
                createSenderMutation.isPending ||
                createForm.value.length < 3 ||
                createForm.kind === ""
              }
              onClick={() =>
                createSenderMutation.mutate({
                  value: createForm.value,
                  kind: createForm.kind,
                  notes: createForm.notes === "" ? undefined : createForm.notes,
                })
              }
            >
              {createSenderMutation.isPending ? "Creating…" : "Create"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* Register with a new provider */}
      <Dialog
        open={registerProviderId !== null}
        onOpenChange={(open) => !open && setRegisterProviderId(null)}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Register {selectedSenderId?.value} with a provider</DialogTitle>
            <DialogDescription>
              Creates a new registration row for this (sender ID, provider) pair, status{" "}
              <span className="font-mono">pending</span>, submitted now.
            </DialogDescription>
          </DialogHeader>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="register-provider">Provider</Label>
            <Select
              {...(registerProviderId !== null ? { value: registerProviderId } : {})}
              onValueChange={(value) => setRegisterProviderId(value)}
            >
              <SelectTrigger id="register-provider">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {unregisteredProviders.map((provider) => (
                  <SelectItem key={provider.id} value={provider.id}>
                    {provider.displayName} ({provider.key})
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
          {createRegistrationMutation.isError && (
            <div className="rounded-sm border border-state-danger-border bg-state-danger-bg px-3 py-2 text-caption text-state-danger-fg">
              Failed: {createRegistrationMutation.error.message}
            </div>
          )}
          <DialogFooter>
            <Button type="button" variant="ghost" onClick={() => setRegisterProviderId(null)}>
              Cancel
            </Button>
            <Button
              type="button"
              disabled={createRegistrationMutation.isPending || registerProviderId === null}
              onClick={submitRegistration}
            >
              {createRegistrationMutation.isPending ? "Registering…" : "Register"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* Edit a registration */}
      <Dialog
        open={editingRegistration !== null}
        onOpenChange={(open) => {
          if (!open) {
            setEditingRegistration(null);
            setRegistrationForm(null);
          }
        }}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>
              Edit registration —{" "}
              {editingRegistration != null &&
                (providerById.get(editingRegistration.providerId)?.displayName ??
                  editingRegistration.providerId)}
            </DialogTitle>
          </DialogHeader>
          {registrationForm != null && (
            <div className="flex flex-col gap-4">
              <div className="flex flex-col gap-1.5">
                <Label htmlFor="registration-status">Status</Label>
                <Select
                  value={registrationForm.status}
                  onValueChange={(value) =>
                    setRegistrationForm({ ...registrationForm, status: value })
                  }
                >
                  <SelectTrigger id="registration-status">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {KNOWN_STATUSES.map((status) => (
                      <SelectItem key={status} value={status}>
                        {status}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
              <div className="flex flex-col gap-1.5">
                <Label htmlFor="registration-reference">
                  Reference (the provider's own tracking id, if any)
                </Label>
                <Input
                  id="registration-reference"
                  value={registrationForm.reference}
                  onChange={(e) =>
                    setRegistrationForm({ ...registrationForm, reference: e.target.value })
                  }
                />
              </div>
              <div className="flex flex-col gap-1.5">
                <Label htmlFor="registration-rejection-reason">
                  Rejection reason (what needs to change before resubmitting)
                </Label>
                <Textarea
                  id="registration-rejection-reason"
                  rows={3}
                  value={registrationForm.rejectionReason}
                  onChange={(e) =>
                    setRegistrationForm({ ...registrationForm, rejectionReason: e.target.value })
                  }
                />
              </div>
              {updateRegistrationMutation.isError && (
                <div className="rounded-sm border border-state-danger-border bg-state-danger-bg px-3 py-2 text-caption text-state-danger-fg">
                  Save failed: {updateRegistrationMutation.error.message}
                </div>
              )}
            </div>
          )}
          <DialogFooter>
            <Button
              type="button"
              variant="ghost"
              onClick={() => {
                setEditingRegistration(null);
                setRegistrationForm(null);
              }}
            >
              Cancel
            </Button>
            <Button
              type="button"
              disabled={updateRegistrationMutation.isPending}
              onClick={saveRegistration}
            >
              {updateRegistrationMutation.isPending ? "Saving…" : "Save"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* Resubmit confirm */}
      <Dialog
        open={resubmitTarget !== null}
        onOpenChange={(open) => !open && setResubmitTarget(null)}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Resubmit this registration?</DialogTitle>
            <DialogDescription>
              {resubmitTarget != null && (
                <>
                  Moves{" "}
                  <span className="font-mono text-foreground">
                    {providerById.get(resubmitTarget.providerId)?.displayName ??
                      resubmitTarget.providerId}
                  </span>{" "}
                  back to <span className="font-mono">pending</span>, stamps a fresh submitted-at,
                  and clears the rejection reason — use this once whatever the provider objected to
                  is actually fixed, not before.
                </>
              )}
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button type="button" variant="ghost" onClick={() => setResubmitTarget(null)}>
              Cancel
            </Button>
            <Button type="button" onClick={confirmResubmit}>
              Resubmit
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </main>
  );
}
