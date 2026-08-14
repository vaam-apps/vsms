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
// Two confirmations on this screen are a short *form*, not a yes/no
// question — "register with a new provider" (a provider picker) and, less
// obviously, the review drawer's own status/reference/rejection-reason
// edit is itself the "form" case. "Register with a provider" and
// "Resubmit this registration?" both need real confirmation weight (§1.7)
// and both are rendered **inline inside the drawer that triggered them**,
// not as a nested `Dialog` — a centered Headless UI `Dialog` nested inside
// an open `vaul` drawer never becomes visible or interactive, a real,
// verified bug, and it reproduces at *any* nesting depth (the registration
// review drawer is itself stacked inside the sender id's own more-detail
// drawer). See `frontends/apps/admin/app/gallery/page.tsx`'s
// `NestedDialogInDrawerRegression` demo and `docs/design/console-redesign.md`
// §3/§1.7 for the mechanism and root cause. Create sender ID stays a real
// centered `Dialog` — it opens from the toolbar while no drawer is open.
//
// # Registration status is a plain `String`, not a governed enum
//
// `@vsms/ui`'s own `Badge` doc: "Never use `Badge` for a message or job
// state — that's `StatusPill`'s job... mixing them is how the status
// language erodes." `SenderIdRegistration.status` genuinely isn't part of
// that governed vocabulary (`schema.cstack` declares it a bare `String`,
// and only `"approved"` is load-bearing anywhere server-side —
// `procedures.rs::resolve_sender_id`'s own `APPROVED` const). So
// `RegistrationStatusBadge` (`./components/registration-status-badge.tsx`)
// renders it as a `Badge` (a tag, exactly what the component is for) with
// a colour hint layered on top locally — never a fake `StatusPill` for a
// value the schema itself never closed into an enum.
//
// # Provider names, not raw ids
//
// `trpc.providers.list` already exists (#54) and needs no new server work —
// this screen joins registrations to it purely for display and to build
// the "register with a new provider" picker's own options (every provider
// that doesn't already have a registration row for the selected sender id).
//
// # R6
//
// This file holds data fetching, mutations, URL/local state, and handlers
// only. Markup and classes live in `./components/*` (route-local — nothing
// here is reused by another screen) and `./sender-id-domain.ts` (form
// schemas and the registration-summary formatter, extracted so they're
// unit-testable without mounting React).

import { zodResolver } from "@hookform/resolvers/zod";
import { trpc } from "@vsms/hooks";
import {
  IdDisplay,
  MoreDetailDrawer,
  QuickDetailDrawer,
  ScreenHeader,
  ScreenStack,
  toast,
} from "@vsms/ui";
import { parseAsString, useQueryState } from "nuqs";
import { useEffect, useMemo, useState } from "react";
import { useForm } from "react-hook-form";
import { CreateSenderDialog } from "./components/create-sender-dialog";
import { RegisterProviderConfirm } from "./components/register-provider-confirm";
import { RegistrationReviewFields } from "./components/registration-review-fields";
import { RegistrationReviewFooter } from "./components/registration-review-footer";
import { ResubmitConfirm } from "./components/resubmit-confirm";
import { SenderEditFooter } from "./components/sender-edit-footer";
import { SenderMoreDetailBody } from "./components/sender-more-detail-body";
import { SenderQuickDetailBody } from "./components/sender-quick-detail-body";
import { SenderQuickDetailFooter } from "./components/sender-quick-detail-footer";
import { SenderTable } from "./components/sender-table";
import { SenderToolbar } from "./components/sender-toolbar";
import {
  type CreateSenderIdFormValues,
  createSenderIdSchema,
  type ProviderListItem,
  type RegistrationFormValues,
  type RegistrationListItem,
  registrationSchema,
  type SenderIdFormValues,
  senderIdSchema,
  summarizeRegistrations,
} from "./sender-id-domain";

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

  function registrationsFor(senderId: string): RegistrationListItem[] {
    return registrationsBySender.get(senderId) ?? [];
  }

  function summaryFor(senderId: string): string {
    return summarizeRegistrations(registrationsFor(senderId), providerById);
  }

  // --- Quick detail (local state, no route — D14) --------------------------

  const [quickId, setQuickId] = useState<string | null>(null);
  const quickDetail = listQuery.data?.find((s) => s.id === quickId);
  const quickRegistrations = quickId != null ? registrationsFor(quickId) : [];

  // --- More detail: the sender ID itself (`?panel=<id>`) -------------------

  const [panelId, setPanelId] = useQueryState("panel", parseAsString);
  const panelTarget = listQuery.data?.find((s) => s.id === panelId);
  const panelRegistrations = panelId != null ? registrationsFor(panelId) : [];
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

  function closeSenderMore() {
    void setPanelId(null);
    setRegisterProviderId(null);
  }

  // --- Create sender ID (Dialog — short, single-purpose) -------------------

  const [createOpen, setCreateOpen] = useState(false);
  const createForm = useForm<CreateSenderIdFormValues>({
    resolver: zodResolver(createSenderIdSchema),
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

  function submitCreate(values: CreateSenderIdFormValues) {
    createSenderMutation.mutate({
      value: values.value,
      kind: values.kind,
      notes: values.notes === "" ? undefined : values.notes,
    });
  }

  // --- Register with a new provider (inline, armed by a non-null id) -------

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

  function closeRegistrationReview() {
    void setRegistrationId(null);
    setResubmitArmed(false);
  }

  // A boolean, not a copy of the row (R6) — resubmit always targets
  // whatever `registrationTarget` the review drawer is currently open on.
  const [resubmitArmed, setResubmitArmed] = useState(false);
  const resubmitMutation = trpc.senderIdRegistrations.update.useMutation({
    onSuccess: () => {
      toast({ title: "Registration resubmitted", variant: "success" });
      setResubmitArmed(false);
      void setRegistrationId(null);
      void utils.senderIdRegistrations.invalidate();
    },
  });

  function confirmResubmit() {
    if (registrationTarget === undefined) return;
    resubmitMutation.mutate({
      id: registrationTarget.id,
      etag: String(registrationTarget.version),
      status: "pending",
      submittedAt: new Date().toISOString(),
      // `""`, not `null` — a verified no-op on this route (module doc).
      rejectionReason: "",
    });
  }

  return (
    <ScreenStack>
      <ScreenHeader
        title="Sender IDs"
        description={
          'Brand identifiers and their per-provider registration status — a sender ID isn\'t "approved" or "rejected" globally, only against one provider at a time.'
        }
      />

      <SenderToolbar
        errorMessage={listQuery.error?.message ?? registrationsQuery.error?.message}
        onNewSenderId={() => setCreateOpen(true)}
      />

      <SenderTable
        senderIds={listQuery.data}
        isLoading={listQuery.isLoading}
        summaryFor={summaryFor}
        onRowClick={(s) => setQuickId(s.id)}
      />

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
          quickDetail !== undefined && (
            <SenderQuickDetailFooter
              onClose={() => setQuickId(null)}
              onViewFullDetails={() => {
                void setPanelId(quickDetail.id);
                setQuickId(null);
              }}
            />
          )
        }
      >
        {quickDetail !== undefined && (
          <SenderQuickDetailBody
            senderId={quickDetail}
            registrations={quickRegistrations}
            providerById={providerById}
            onReviewRegistration={(id) => void setRegistrationId(id)}
          />
        )}
      </QuickDetailDrawer>

      {/* More detail — the sender id's own edit form plus the full
          registrations table (D14). Body/footer swap to the inline
          "register with a new provider" form when armed. */}
      <MoreDetailDrawer
        open={panelId !== null}
        onOpenChange={(open) => !open && closeSenderMore()}
        title={panelTarget?.value ?? "Sender ID"}
        description={
          panelTarget !== undefined && <IdDisplay value={panelTarget.id} variant="full" />
        }
        footer={
          registerProviderId !== null
            ? undefined
            : panelTarget !== undefined && (
                <SenderEditFooter
                  pending={updateSenderMutation.isPending}
                  onClose={() => void setPanelId(null)}
                />
              )
        }
      >
        {panelTarget !== undefined &&
          (registerProviderId !== null ? (
            <RegisterProviderConfirm
              senderIdValue={panelTarget.value}
              unregisteredProviders={unregisteredProviders}
              selectedProviderId={registerProviderId}
              onSelectProvider={setRegisterProviderId}
              pending={createRegistrationMutation.isPending}
              errorMessage={createRegistrationMutation.error?.message}
              onConfirm={submitRegistration}
              onCancel={() => setRegisterProviderId(null)}
            />
          ) : (
            <SenderMoreDetailBody
              formId="sender-id-edit-form"
              form={senderForm}
              onSubmit={saveSender}
              saveErrorMessage={updateSenderMutation.error?.message}
              registrations={panelRegistrations}
              providerById={providerById}
              canRegisterMore={unregisteredProviders.length > 0}
              onRegisterNew={() => setRegisterProviderId(unregisteredProviders[0]?.id ?? null)}
              onReviewRegistration={(id) => void setRegistrationId(id)}
            />
          ))}
      </MoreDetailDrawer>

      {/* Registration review — a second, stacked more-detail drawer (D14,
          §3's own worked example for this exact action). Body/footer swap
          to the inline resubmit confirmation when armed. */}
      <MoreDetailDrawer
        open={registrationId !== null}
        onOpenChange={(open) => !open && closeRegistrationReview()}
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
          resubmitArmed
            ? undefined
            : registrationTarget !== undefined && (
                <RegistrationReviewFooter
                  showResubmit={registrationTarget.status === "rejected"}
                  pending={updateRegistrationMutation.isPending}
                  onResubmit={() => setResubmitArmed(true)}
                  onClose={() => void setRegistrationId(null)}
                />
              )
        }
      >
        {registrationTarget !== undefined &&
          (resubmitArmed ? (
            <ResubmitConfirm
              registration={registrationTarget}
              providerById={providerById}
              pending={resubmitMutation.isPending}
              onConfirm={confirmResubmit}
              onCancel={() => setResubmitArmed(false)}
            />
          ) : (
            <RegistrationReviewFields
              formId="registration-review-form"
              form={registrationForm}
              onSubmit={saveRegistration}
              saveErrorMessage={updateRegistrationMutation.error?.message}
            />
          ))}
      </MoreDetailDrawer>

      {/* Create sender ID — short, single-purpose form (§3). Not affected
          by the nested-Dialog-in-drawer bug: it opens from the toolbar
          while no drawer is open. */}
      <CreateSenderDialog
        open={createOpen}
        onOpenChange={(open) => !open && setCreateOpen(false)}
        form={createForm}
        onSubmit={submitCreate}
        pending={createSenderMutation.isPending}
        errorMessage={createSenderMutation.error?.message}
      />
    </ScreenStack>
  );
}
