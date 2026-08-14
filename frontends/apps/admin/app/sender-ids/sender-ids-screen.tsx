"use client";

// The Sender IDs screen (#53): sender ID CRUD plus per-provider registration
// status, with rejection reasons front and centre. "Sender IDs need
// registration per provider, and the status is per (sender ID, provider)
// pair rather than global. Surface rejection reasons — they are
// actionable" — the issue's own words.
//
// # Quick vs. more detail, and a second, nested more-detail (console-
// redesign.md §3/D14)
//
// A sender ID row opens `QuickDetailDrawer` (value/kind/active + a
// registrations mini-list, each with its own "Review" action) with a "View
// full details" action that upgrades to `MoreDetailDrawer` (`?panel=<id>`)
// — the sender ID's own edit form plus the full registrations table. Per
// §3's own worked example ("Sender ID registration review/approve-reject
// ... becomes a more-details drawer"), reviewing one registration opens a
// *second*, independent `MoreDetailDrawer` (`?registration=<id>`) stacked
// over whichever drawer it was opened from — reachable from either the
// quick peek or the full sender-ID drawer, so an operator never has to
// open the wide drawer just to approve/reject one registration.
//
// Create sender ID and "register with a new provider" stay short, single-
// purpose `Dialog`s (§3: "short single-purpose forms with no sub-
// navigation") — resubmitting a rejected registration is the same shape,
// nested inside the registration review drawer.
//
// # Registration status is a plain `String`, not a governed enum
//
// `@vsms/ui`'s own `Badge` doc: "Never use `Badge` for a message or job
// state — that's `StatusPill`'s job... mixing them is how the status
// language erodes." `SenderIdRegistration.status` genuinely isn't part of
// that governed vocabulary (`schema.cstack` declares it a bare `String`,
// and only `"approved"` is load-bearing anywhere server-side —
// `procedures.rs::resolve_sender_id`'s own `APPROVED` const). So this
// screen renders it as a `Badge` (a tag, exactly what the component is
// for) with a colour hint layered on top locally — never a fake
// `StatusPill` for a value the schema itself never closed into an enum.
//
// # Provider names, not raw ids
//
// `trpc.providers.list` already exists (#54) and needs no new server work —
// this screen joins registrations to it purely for display (`providerName`)
// and to build the "register with a new provider" picker's own options
// (every provider that doesn't already have a registration row for the
// selected sender id).

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
  Textarea,
  TimestampDisplay,
  toast,
} from "@vsms/ui";
import { parseAsString, useQueryState } from "nuqs";
import { useEffect, useMemo, useState } from "react";
import { Controller, useForm } from "react-hook-form";
import { z } from "zod";

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

const senderIdSchema = z.object({
  value: z.string().trim().min(3, "3–11 characters").max(11, "3–11 characters"),
  kind: z.string().trim().min(1, "Kind is required"),
  notes: z.string(),
  active: z.boolean(),
});
type SenderIdFormValues = z.infer<typeof senderIdSchema>;

const registrationSchema = z.object({
  status: z.string().min(1),
  reference: z.string(),
  rejectionReason: z.string(),
});
type RegistrationFormValues = z.infer<typeof registrationSchema>;

const createSchema = z.object({
  value: z.string().trim().min(3, "3–11 characters").max(11, "3–11 characters"),
  kind: z.string().trim().min(1, "Kind is required"),
  notes: z.string(),
});
type CreateFormValues = z.infer<typeof createSchema>;

export function SenderIdsScreen() {
  const utils = trpc.useUtils();
  const listQuery = trpc.senderIds.list.useQuery();
  const registrationsQuery = trpc.senderIdRegistrations.list.useQuery();
  const providersQuery = trpc.providers.list.useQuery();

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

  function summarize(senderId: string): string {
    const registrations = registrationsBySender.get(senderId) ?? [];
    if (registrations.length === 0) return "not registered anywhere";
    return registrations
      .map((r) => `${providerById.get(r.providerId)?.key ?? r.providerId}: ${r.status}`)
      .join(" · ");
  }

  // --- Quick detail (local state, no route — D14) --------------------------

  const [quickId, setQuickId] = useState<string | null>(null);
  const quickDetail = listQuery.data?.find((s) => s.id === quickId);
  const quickRegistrations = quickId != null ? (registrationsBySender.get(quickId) ?? []) : [];

  // --- More detail: the sender ID itself (`?panel=<id>`) -------------------

  const [panelId, setPanelId] = useQueryState("panel", parseAsString);
  const panelTarget = listQuery.data?.find((s) => s.id === panelId);
  const panelRegistrations = panelId != null ? (registrationsBySender.get(panelId) ?? []) : [];
  const registeredProviderIds = new Set(panelRegistrations.map((r) => r.providerId));
  const unregisteredProviders = (providersQuery.data ?? []).filter(
    (p) => !registeredProviderIds.has(p.id),
  );

  const senderForm = useForm<SenderIdFormValues>({ resolver: zodResolver(senderIdSchema) });

  // biome-ignore lint/correctness/useExhaustiveDependencies: `senderForm` is stable across renders.
  useEffect(() => {
    if (panelTarget === undefined) return;
    senderForm.reset({
      value: panelTarget.value,
      kind: panelTarget.kind,
      notes: panelTarget.notes ?? "",
      active: panelTarget.active,
    });
  }, [panelTarget]);

  const updateSenderMutation = trpc.senderIds.update.useMutation({
    onSuccess: () => {
      toast({ title: "Sender ID saved", variant: "success" });
      void utils.senderIds.list.invalidate();
    },
  });

  function saveSender(values: SenderIdFormValues) {
    if (panelTarget === undefined) return;
    updateSenderMutation.mutate({
      id: panelTarget.id,
      etag: String(panelTarget.version),
      value: values.value,
      kind: values.kind,
      // An empty string here is a real, sent value ("clear notes"), not an
      // omitted key — see `@vsms/gateway/senders.ts`'s own module doc: a
      // verified cratestack-macros gap means `undefined`/`null` cannot
      // clear a nullable column over this REST route, only a genuine `""`.
      notes: values.notes,
      active: values.active,
    });
  }

  // --- Create sender ID (Dialog — short, single-purpose) -------------------

  const [createOpen, setCreateOpen] = useState(false);
  const createForm = useForm<CreateFormValues>({
    resolver: zodResolver(createSchema),
    defaultValues: { value: "", kind: "", notes: "" },
  });
  const createSenderMutation = trpc.senderIds.create.useMutation({
    onSuccess: (created) => {
      toast({ title: "Sender ID created", variant: "success" });
      setCreateOpen(false);
      createForm.reset({ value: "", kind: "", notes: "" });
      void utils.senderIds.list.invalidate();
      void setPanelId(created.id);
    },
  });

  function submitCreate(values: CreateFormValues) {
    createSenderMutation.mutate({
      value: values.value,
      kind: values.kind,
      notes: values.notes === "" ? undefined : values.notes,
    });
  }

  // --- Register with a new provider (Dialog) --------------------------------

  const [registerProviderId, setRegisterProviderId] = useState<string | null>(null);
  const createRegistrationMutation = trpc.senderIdRegistrations.create.useMutation({
    onSuccess: () => {
      toast({ title: "Registration created", variant: "success" });
      setRegisterProviderId(null);
      void utils.senderIdRegistrations.list.invalidate();
    },
  });

  function submitRegistration() {
    if (panelId === null || registerProviderId === null) return;
    createRegistrationMutation.mutate({
      senderIdId: panelId,
      providerId: registerProviderId,
      status: "pending",
    });
  }

  // --- Registration review: a second, stacked more-detail drawer
  //     (`?registration=<id>`) --------------------------------------------

  const [registrationId, setRegistrationId] = useQueryState("registration", parseAsString);
  const registrationTarget = registrationsQuery.data?.find((r) => r.id === registrationId);

  const registrationForm = useForm<RegistrationFormValues>({
    resolver: zodResolver(registrationSchema),
  });

  // biome-ignore lint/correctness/useExhaustiveDependencies: `registrationForm` is stable across renders.
  useEffect(() => {
    if (registrationTarget === undefined) return;
    registrationForm.reset({
      status: registrationTarget.status,
      reference: registrationTarget.reference ?? "",
      rejectionReason: registrationTarget.rejectionReason ?? "",
    });
  }, [registrationTarget]);

  const updateRegistrationMutation = trpc.senderIdRegistrations.update.useMutation({
    onSuccess: () => {
      toast({ title: "Registration saved", variant: "success" });
      void setRegistrationId(null);
      void utils.senderIdRegistrations.invalidate();
    },
  });

  function saveRegistration(values: RegistrationFormValues) {
    if (registrationTarget === undefined) return;
    updateRegistrationMutation.mutate({
      id: registrationTarget.id,
      etag: String(registrationTarget.version),
      status: values.status,
      // Empty strings, sent as-is — see `saveSender`'s own comment above.
      reference: values.reference,
      rejectionReason: values.rejectionReason,
    });
  }

  const [resubmitTarget, setResubmitTarget] = useState<RegistrationListItem | null>(null);
  const resubmitMutation = trpc.senderIdRegistrations.update.useMutation({
    onSuccess: () => {
      toast({ title: "Registration resubmitted", variant: "success" });
      setResubmitTarget(null);
      void setRegistrationId(null);
      void utils.senderIdRegistrations.invalidate();
    },
  });

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

  return (
    <main className="mx-auto flex max-w-[1400px] flex-col gap-6 px-4 py-6 sm:px-6 sm:py-10">
      <header className="flex flex-col gap-1 border-edge border-b pb-6">
        <p className="font-mono text-micro text-subtle-foreground tracking-[0.03em]">
          vsms admin console
        </p>
        <h1 className="font-medium text-foreground text-title">Sender IDs</h1>
        <p className="max-w-xl text-body text-muted-foreground">
          Brand identifiers and their per-provider registration status — a sender ID isn't
          "approved" or "rejected" globally, only against one provider at a time.
        </p>
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
            <TableHead className="hidden sm:table-cell">Kind</TableHead>
            <TableHead className="hidden md:table-cell">Registrations</TableHead>
            <TableHead align="end" className="hidden md:table-cell">
              Updated
            </TableHead>
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
              onClick={() => setQuickId(senderId.id)}
            >
              <TableCell>
                {senderId.active ? (
                  <span className="text-state-success-fg">active</span>
                ) : (
                  <span className="text-muted-foreground">inactive</span>
                )}
              </TableCell>
              <TableCell mono>{senderId.value}</TableCell>
              <TableCell mono className="hidden sm:table-cell">
                {senderId.kind}
              </TableCell>
              <TableCell className="hidden md:table-cell">
                <span className="text-caption text-muted-foreground">{summarize(senderId.id)}</span>
              </TableCell>
              <TableCell align="end" className="hidden md:table-cell">
                <TimestampDisplay value={senderId.updatedAt} />
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>

      {/* Quick detail — a peek at the sender id plus its registrations,
          with a per-registration Review shortcut. */}
      <QuickDetailDrawer
        open={quickId !== null}
        onOpenChange={(open) => !open && setQuickId(null)}
        title={quickDetail?.value ?? "Sender ID"}
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
              View full details
            </Button>
          </>
        }
      >
        {quickDetail !== undefined && (
          <div className="flex flex-col gap-4">
            <dl className="flex flex-col gap-3 text-body">
              <div className="flex items-center justify-between gap-3">
                <dt className="text-muted-foreground">Active</dt>
                <dd>{quickDetail.active ? "yes" : "no"}</dd>
              </div>
              <div className="flex items-center justify-between gap-3">
                <dt className="text-muted-foreground">Kind</dt>
                <dd className="font-mono text-caption">{quickDetail.kind}</dd>
              </div>
              {quickDetail.notes != null && quickDetail.notes !== "" && (
                <div className="flex flex-col gap-1">
                  <dt className="text-muted-foreground">Notes</dt>
                  <dd className="text-caption">{quickDetail.notes}</dd>
                </div>
              )}
            </dl>

            <div className="flex flex-col gap-2 border-edge border-t pt-3">
              <p className="text-caption text-subtle-foreground">Registrations</p>
              {quickRegistrations.length === 0 ? (
                <InlineEmptyState message="Not registered with any provider yet." />
              ) : (
                quickRegistrations.map((registration) => (
                  <div
                    key={registration.id}
                    className="flex items-center justify-between gap-2 rounded-sm border border-edge px-2 py-1.5"
                  >
                    <div className="flex min-w-0 items-center gap-2">
                      <span className="truncate font-mono text-caption">
                        {providerById.get(registration.providerId)?.key ?? registration.providerId}
                      </span>
                      <RegistrationStatusBadge status={registration.status} />
                    </div>
                    <Button
                      type="button"
                      variant="ghost"
                      size="sm"
                      onClick={() => void setRegistrationId(registration.id)}
                    >
                      Review
                    </Button>
                  </div>
                ))
              )}
            </div>
          </div>
        )}
      </QuickDetailDrawer>

      {/* More detail — the sender id's own edit form plus the full
          registrations table (D14). */}
      <MoreDetailDrawer
        open={panelId !== null}
        onOpenChange={(open) => !open && void setPanelId(null)}
        title={panelTarget?.value ?? "Sender ID"}
        description={
          panelTarget !== undefined && <IdDisplay value={panelTarget.id} variant="full" />
        }
        footer={
          <>
            <Button type="button" variant="ghost" onClick={() => void setPanelId(null)}>
              Close
            </Button>
            <Button
              type="submit"
              form="sender-id-edit-form"
              disabled={updateSenderMutation.isPending}
            >
              {updateSenderMutation.isPending ? "Saving…" : "Save"}
            </Button>
          </>
        }
      >
        {panelTarget !== undefined && (
          <div className="flex flex-col gap-6">
            <form
              id="sender-id-edit-form"
              onSubmit={senderForm.handleSubmit(saveSender)}
              className="flex flex-col gap-4"
            >
              <div className="flex flex-col gap-1.5">
                <Label htmlFor="sender-value">Value</Label>
                <Input
                  id="sender-value"
                  aria-invalid={senderForm.formState.errors.value != null}
                  {...senderForm.register("value")}
                />
                {senderForm.formState.errors.value != null && (
                  <p className="text-caption text-state-danger-fg">
                    {senderForm.formState.errors.value.message}
                  </p>
                )}
              </div>
              <div className="flex flex-col gap-1.5">
                <Label htmlFor="sender-kind">Kind</Label>
                <Input
                  id="sender-kind"
                  placeholder="e.g. alphanumeric"
                  {...senderForm.register("kind")}
                />
              </div>
              <div className="flex flex-col gap-1.5">
                <Label htmlFor="sender-notes">Notes</Label>
                <Input id="sender-notes" {...senderForm.register("notes")} />
              </div>
              <Controller
                control={senderForm.control}
                name="active"
                render={({ field }) => (
                  <label className="flex items-center gap-2 text-body text-foreground">
                    <input
                      type="checkbox"
                      checked={field.value}
                      onChange={(e) => field.onChange(e.target.checked)}
                      className="checkbox"
                    />
                    Active — eligible for <span className="font-mono">sendMessage</span> to resolve
                    as a default or explicit sender
                  </label>
                )}
              />
              {updateSenderMutation.isError && (
                <div className="rounded-sm border border-state-danger-border bg-state-danger-bg px-3 py-2 text-caption text-state-danger-fg">
                  Save failed: {updateSenderMutation.error.message}
                </div>
              )}
            </form>

            <div className="flex flex-col gap-3 border-edge border-t pt-4">
              <div className="flex items-center justify-between">
                <h3 className="font-medium text-body text-foreground">
                  Registrations, per provider
                </h3>
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

              {panelRegistrations.length === 0 ? (
                <InlineEmptyState message="Not registered with any provider yet." />
              ) : (
                <div className="flex flex-col gap-2">
                  {panelRegistrations.map((registration) => (
                    <div
                      key={registration.id}
                      className="flex flex-col gap-2 rounded-sm border border-edge p-3 sm:flex-row sm:items-center sm:justify-between"
                    >
                      <div className="flex flex-col gap-1">
                        <div className="flex items-center gap-2">
                          <span className="font-mono text-body text-foreground">
                            {providerById.get(registration.providerId)?.displayName ??
                              registration.providerId}
                          </span>
                          <RegistrationStatusBadge status={registration.status} />
                        </div>
                        {registration.rejectionReason != null && (
                          <p className="text-caption text-state-danger-fg">
                            {registration.rejectionReason}
                          </p>
                        )}
                      </div>
                      <Button
                        type="button"
                        variant="ghost"
                        size="sm"
                        onClick={() => void setRegistrationId(registration.id)}
                      >
                        Review
                      </Button>
                    </div>
                  ))}
                </div>
              )}
            </div>
          </div>
        )}
      </MoreDetailDrawer>

      {/* Registration review — a second, stacked more-detail drawer (D14,
          §3's own worked example for this exact action). */}
      <MoreDetailDrawer
        open={registrationId !== null}
        onOpenChange={(open) => !open && void setRegistrationId(null)}
        title="Registration review"
        description={
          registrationTarget !== undefined && (
            <>
              {providerById.get(registrationTarget.providerId)?.displayName ??
                registrationTarget.providerId}{" "}
              · <IdDisplay value={registrationTarget.id} variant="full" />
            </>
          )
        }
        footer={
          <>
            {registrationTarget?.status === "rejected" && (
              <Button
                type="button"
                variant="secondary"
                size="sm"
                className="mr-auto"
                onClick={() => setResubmitTarget(registrationTarget)}
              >
                Resubmit
              </Button>
            )}
            <Button type="button" variant="ghost" onClick={() => void setRegistrationId(null)}>
              Close
            </Button>
            <Button
              type="submit"
              form="registration-review-form"
              disabled={updateRegistrationMutation.isPending}
            >
              {updateRegistrationMutation.isPending ? "Saving…" : "Save"}
            </Button>
          </>
        }
      >
        {registrationTarget !== undefined && (
          <form
            id="registration-review-form"
            onSubmit={registrationForm.handleSubmit(saveRegistration)}
            className="flex flex-col gap-4"
          >
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="registration-status">Status</Label>
              <Controller
                control={registrationForm.control}
                name="status"
                render={({ field }) => (
                  <Select value={field.value} onValueChange={field.onChange}>
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
                )}
              />
            </div>
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="registration-reference">
                Reference (the provider's own tracking id, if any)
              </Label>
              <Input id="registration-reference" {...registrationForm.register("reference")} />
            </div>
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="registration-rejection-reason">
                Rejection reason (what needs to change before resubmitting)
              </Label>
              <Textarea
                id="registration-rejection-reason"
                rows={3}
                {...registrationForm.register("rejectionReason")}
              />
            </div>
            {updateRegistrationMutation.isError && (
              <div className="rounded-sm border border-state-danger-border bg-state-danger-bg px-3 py-2 text-caption text-state-danger-fg">
                Save failed: {updateRegistrationMutation.error.message}
              </div>
            )}
          </form>
        )}
      </MoreDetailDrawer>

      {/* Create sender ID — short, single-purpose form (§3). */}
      <Dialog open={createOpen} onOpenChange={(open) => !open && setCreateOpen(false)}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>New sender ID</DialogTitle>
            <DialogDescription>
              Created inactive — activate it from the detail drawer once it's ready to be used.
            </DialogDescription>
          </DialogHeader>
          <form
            id="create-sender-id-form"
            onSubmit={createForm.handleSubmit(submitCreate)}
            className="flex flex-col gap-4"
          >
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="new-sender-value">Value (3–11 characters)</Label>
              <Input
                id="new-sender-value"
                aria-invalid={createForm.formState.errors.value != null}
                {...createForm.register("value")}
              />
              {createForm.formState.errors.value != null && (
                <p className="text-caption text-state-danger-fg">
                  {createForm.formState.errors.value.message}
                </p>
              )}
            </div>
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="new-sender-kind">Kind</Label>
              <Input
                id="new-sender-kind"
                placeholder="e.g. alphanumeric"
                {...createForm.register("kind")}
              />
            </div>
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="new-sender-notes">Notes (optional)</Label>
              <Input id="new-sender-notes" {...createForm.register("notes")} />
            </div>
            {createSenderMutation.isError && (
              <div className="rounded-sm border border-state-danger-border bg-state-danger-bg px-3 py-2 text-caption text-state-danger-fg">
                Create failed: {createSenderMutation.error.message}
              </div>
            )}
          </form>
          <DialogFooter>
            <Button type="button" variant="ghost" onClick={() => setCreateOpen(false)}>
              Cancel
            </Button>
            <Button
              type="submit"
              form="create-sender-id-form"
              disabled={createSenderMutation.isPending}
            >
              {createSenderMutation.isPending ? "Creating…" : "Create"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* Register with a new provider (Dialog). */}
      <Dialog
        open={registerProviderId !== null}
        onOpenChange={(open) => !open && setRegisterProviderId(null)}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Register {panelTarget?.value} with a provider</DialogTitle>
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

      {/* Resubmit confirm — nested inside the registration review drawer,
          a short confirmation, stays a Dialog (§3's own footnote: "a form
          can open a confirmation without contradicting the rule above"). */}
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
