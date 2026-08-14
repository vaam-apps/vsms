"use client";

// The Webhooks screen (#55): endpoint CRUD, attempt history with status
// codes and errors, one-click replay, and secret rotation with the overlap
// window made visible. "Endpoint CRUD, attempt history with status codes
// and errors, one-click replay, and rotation with the overlap window made
// visible" — the issue's own words, verbatim.
//
// # Quick vs. more detail (console-redesign.md §3/D14)
//
// An endpoint row opens `QuickDetailDrawer` (URL, active, circuit state,
// event count, the current secret masked) with a "View full details"
// action that upgrades to `MoreDetailDrawer` (`?panel=<id>`) — the edit
// form, event-type toggles, and secret reveal/rotate. A delivery attempt
// row opens its own `QuickDetailDrawer` (state/event/endpoint/attempts/
// status/error, a Replay action) with "View payload" upgrading to a second
// `MoreDetailDrawer` (`?attempt=<id>`) holding `PayloadInspector` — the
// same "peek, then upgrade" shape §3 asks for, applied to a second record
// type on this same screen.
//
// Delete, secret rotation, and replay are all irreversible-enough to need
// a confirmation step (§1.7) — every one of them is rendered **inline
// inside the drawer they were triggered from**, not as a nested `Dialog`:
// a centered Headless UI `Dialog` nested inside an open `vaul` drawer never
// becomes visible or interactive, a real, verified bug. See
// `frontends/apps/admin/app/gallery/page.tsx`'s
// `NestedDialogInDrawerRegression` demo and `docs/design/console-redesign.md`
// §3/§1.7 for the mechanism and root cause. The one *unaffected* `Dialog`
// on this screen is "New webhook endpoint" — triggered from the toolbar
// while no drawer is open.
//
// # `WebhookEndpoint.secret`/`prevSecret` are shown, masked by default, with
// an explicit Reveal — see `@vsms/gateway/webhooks.ts`'s own module doc for
// the full reasoning (`@sensitive` doesn't redact API responses, only audit
// snapshots — #187's read policy, `owner`/`admin`/`developer`/`system`, is
// the only real control, and it already ran before this screen ever
// receives the value). Masking here is a screen-share/shoulder-surf
// discipline, not a pretend security boundary.
//
// # Rotation is a destructive action with a delay fuse, not an instant one
//
// Rotating does not invalidate the current secret immediately — it starts
// an *overlap window* with no fixed end: the just-rotated-away secret keeps
// verifying as `prevSecret` until the NEXT rotation, whenever that happens
// (`sms_webhook::verify` tries every candidate; #59's own doc on
// `rotateWebhookSecret`). The confirm panel below says this before the
// click, not after, and the immediate post-rotation view keeps both values
// visible side by side with `secretRotatedAt` — "can I safely stop
// accepting the old one yet?" is the operational question this whole
// feature exists to answer, and hiding the window would be exactly the
// failure mode the issue calls out.
//
// # Replay is a re-send, not a retry-in-place
//
// `replayWebhookAttempt`'s own confirm names the endpoint URL and states
// plainly that the payload is whatever `webhooks.rs`'s subscriber captured
// back when the event first fired — possibly months old (§8.5's own backoff
// schedule tops out at 24h before `dead`, and a `dead` row can sit
// unreplayed indefinitely after that) — not a fresh snapshot of the
// message's current state.
//
// # R6
//
// This file holds data fetching, mutations, URL/local state, and handlers
// only. Markup and classes live in `./components/*` (route-local — nothing
// here is reused by another screen) and `./webhook-domain.ts` (event-type
// vocabulary, form schemas, `maskSecret`/`payloadFor`, extracted so they're
// unit-testable without mounting React).

import { zodResolver } from "@hookform/resolvers/zod";
import { trpc } from "@vsms/hooks";
import {
  ATTEMPT_STATES,
  type AttemptState,
  Button,
  IdDisplay,
  MoreDetailDrawer,
  PayloadInspector,
  QuickDetailDrawer,
  ScreenHeader,
  ScreenStack,
  toast,
} from "@vsms/ui";
import { parseAsString, parseAsStringEnum, useQueryState, useQueryStates } from "nuqs";
import { useState } from "react";
import { useForm } from "react-hook-form";
import { AttemptQuickDetailBody } from "./components/attempt-quick-detail-body";
import { AttemptQuickDetailFooter } from "./components/attempt-quick-detail-footer";
import { AttemptReplayConfirm } from "./components/attempt-replay-confirm";
import { AttemptsTable } from "./components/attempts-table";
import { AttemptsToolbar } from "./components/attempts-toolbar";
import { CreateEndpointDialog } from "./components/create-endpoint-dialog";
import { EndpointDeleteConfirm } from "./components/endpoint-delete-confirm";
import { EndpointEditFooter } from "./components/endpoint-edit-footer";
import { EndpointMoreDetailBody } from "./components/endpoint-more-detail-body";
import { EndpointQuickDetailBody } from "./components/endpoint-quick-detail-body";
import { EndpointRotateConfirm } from "./components/endpoint-rotate-confirm";
import { EndpointTable } from "./components/endpoint-table";
import { EndpointToolbar } from "./components/endpoint-toolbar";
import {
  type CreateEndpointFormValues,
  createEndpointSchema,
  type EndpointFormValues,
  type EventType,
  endpointSchema,
  payloadFor,
} from "./webhook-domain";

export function WebhooksScreen({
  attemptsRefetchIntervalMs,
}: {
  attemptsRefetchIntervalMs: number;
}) {
  const utils = trpc.useUtils();
  const endpointsQuery = trpc.webhookEndpoints.list.useQuery();

  // --- Endpoints: quick + more detail ---------------------------------

  const [quickId, setQuickId] = useState<string | null>(null);
  const quickDetail = endpointsQuery.data?.find((e) => e.id === quickId);

  const [panelId, setPanelId] = useQueryState("panel", parseAsString);
  const panelTarget = endpointsQuery.data?.find((e) => e.id === panelId);

  const [eventTypes, setEventTypes] = useState<EventType[]>([]);
  const form = useForm<EndpointFormValues>({ resolver: zodResolver(endpointSchema) });

  function openMore(endpoint: NonNullable<typeof quickDetail>) {
    void setPanelId(endpoint.id);
    setEventTypes(endpoint.eventTypes);
    form.reset({
      url: endpoint.url,
      maxAttempts: String(endpoint.maxAttempts),
      maskRecipient: endpoint.maskRecipient,
      active: endpoint.active,
    });
    setJustCreatedSecret(null);
    setJustRotatedSecret(null);
  }

  const [justCreatedSecret, setJustCreatedSecret] = useState<string | null>(null);
  const [justRotatedSecret, setJustRotatedSecret] = useState<string | null>(null);

  const updateMutation = trpc.webhookEndpoints.update.useMutation({
    onSuccess: () => {
      toast({ title: "Endpoint saved", variant: "success" });
      void utils.webhookEndpoints.list.invalidate();
    },
  });

  function saveEndpoint(values: EndpointFormValues) {
    if (panelTarget === undefined) return;
    updateMutation.mutate({
      id: panelTarget.id,
      etag: String(panelTarget.version),
      url: values.url,
      eventTypes,
      maskRecipient: values.maskRecipient,
      active: values.active,
      maxAttempts: Number(values.maxAttempts),
    });
  }

  function closeMore() {
    void setPanelId(null);
    setDeleteTargetId(null);
    setRotateArmed(false);
  }

  const deleteMutation = trpc.webhookEndpoints.remove.useMutation({
    onSuccess: () => {
      toast({ title: "Endpoint deleted", variant: "success" });
      setDeleteTargetId(null);
      closeMore();
      void utils.webhookEndpoints.list.invalidate();
      void utils.webhookAttempts.list.invalidate();
    },
  });
  // An id, not a copy of the row (R6) — derived from the live query below,
  // same shape routes-screen.tsx already uses.
  const [deleteTargetId, setDeleteTargetId] = useState<string | null>(null);
  const deleteTarget = endpointsQuery.data?.find((e) => e.id === deleteTargetId);

  // Rotation always targets whatever endpoint the more-detail drawer is
  // currently open on — a boolean "is the confirm armed" flag, not a second
  // copy of the endpoint the way the original centered-`Dialog` version's
  // own `rotateTarget: EndpointListItem | null` was.
  const [rotateArmed, setRotateArmed] = useState(false);
  const rotateMutation = trpc.webhookEndpoints.rotateSecret.useMutation({
    onSuccess: (updated) => {
      toast({ title: "Secret rotated", variant: "success" });
      setRotateArmed(false);
      setJustRotatedSecret(updated.secret);
      void utils.webhookEndpoints.list.invalidate();
    },
  });

  const [createOpen, setCreateOpen] = useState(false);
  const [createEventTypes, setCreateEventTypes] = useState<EventType[]>([]);
  const createForm = useForm<CreateEndpointFormValues>({
    resolver: zodResolver(createEndpointSchema),
    defaultValues: { appId: "", url: "", maxAttempts: "8", maskRecipient: true },
  });
  const createMutation = trpc.webhookEndpoints.create.useMutation({
    onSuccess: (created) => {
      toast({ title: "Endpoint created", variant: "success" });
      setCreateOpen(false);
      createForm.reset({ appId: "", url: "", maxAttempts: "8", maskRecipient: true });
      setCreateEventTypes([]);
      void utils.webhookEndpoints.list.invalidate();
      void setPanelId(created.endpoint.id);
      setEventTypes(created.endpoint.eventTypes);
      form.reset({
        url: created.endpoint.url,
        maxAttempts: String(created.endpoint.maxAttempts),
        maskRecipient: created.endpoint.maskRecipient,
        active: created.endpoint.active,
      });
      setJustCreatedSecret(created.secret);
      setJustRotatedSecret(null);
    },
  });

  function submitCreate(values: CreateEndpointFormValues) {
    createMutation.mutate({
      appId: values.appId,
      url: values.url,
      eventTypes: createEventTypes,
      maskRecipient: values.maskRecipient,
      maxAttempts: Number(values.maxAttempts),
    });
  }

  // --- Attempts: quick + more detail -----------------------------------

  const [filters, setFilters] = useQueryStates(
    {
      endpointId: parseAsString,
      state: parseAsStringEnum<AttemptState>([...ATTEMPT_STATES]),
    },
    { history: "push" },
  );

  const attemptsQuery = trpc.webhookAttempts.list.useQuery(
    {
      endpointId: filters.endpointId ?? undefined,
      state: filters.state ?? undefined,
      limit: 200,
    },
    { refetchInterval: attemptsRefetchIntervalMs },
  );

  const replayMutation = trpc.webhookAttempts.replay.useMutation({
    onSuccess: () => {
      void utils.webhookAttempts.list.invalidate();
    },
  });
  // A boolean, not a copy of the row — same reasoning as `rotateArmed`
  // above: replay always targets whatever `quickAttempt` is currently open.
  const [replayArmed, setReplayArmed] = useState(false);
  const [quickAttemptId, setQuickAttemptId] = useState<string | null>(null);
  const quickAttempt = attemptsQuery.data?.items.find((a) => a.id === quickAttemptId);
  const [attemptPanelId, setAttemptPanelId] = useQueryState("attempt", parseAsString);
  const attemptPanelTarget = attemptsQuery.data?.items.find((a) => a.id === attemptPanelId);

  function endpointUrlFor(endpointId: string): string {
    return endpointsQuery.data?.find((e) => e.id === endpointId)?.url ?? endpointId;
  }

  function confirmReplay() {
    if (quickAttempt === undefined) return;
    replayMutation.mutate({ attemptId: quickAttempt.id });
    setReplayArmed(false);
  }

  return (
    <ScreenStack>
      <ScreenHeader
        title="Webhooks"
        description="Endpoints, delivery attempts, and secret rotation."
      />

      <EndpointToolbar
        listErrorMessage={endpointsQuery.error?.message}
        onNewEndpoint={() => setCreateOpen(true)}
      />

      <EndpointTable
        endpoints={endpointsQuery.data}
        isLoading={endpointsQuery.isLoading}
        onRowClick={(e) => setQuickId(e.id)}
      />

      {/* Quick detail — a peek, no route (D14). */}
      <QuickDetailDrawer
        open={quickId !== null}
        onOpenChange={(open) => !open && setQuickId(null)}
        title={quickDetail?.url ?? "Endpoint"}
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
                openMore(quickDetail);
                setQuickId(null);
              }}
            >
              View full details
            </Button>
          </>
        }
      >
        {quickDetail !== undefined && <EndpointQuickDetailBody endpoint={quickDetail} />}
      </QuickDetailDrawer>

      {/* More detail — the endpoint's own edit form, secret reveal/rotate,
          and Delete (D14). Body/footer swap to whichever inline
          confirmation (delete or rotate) is currently armed. */}
      <MoreDetailDrawer
        open={panelId !== null}
        onOpenChange={(open) => !open && closeMore()}
        title={panelTarget?.url ?? "Endpoint"}
        description={
          panelTarget !== undefined && <IdDisplay value={panelTarget.id} variant="full" />
        }
        footer={
          deleteTarget !== undefined || rotateArmed
            ? undefined
            : panelTarget !== undefined && (
                <EndpointEditFooter
                  pending={updateMutation.isPending}
                  onDelete={() => setDeleteTargetId(panelTarget.id)}
                  onClose={closeMore}
                />
              )
        }
      >
        {deleteTarget !== undefined ? (
          <EndpointDeleteConfirm
            endpoint={deleteTarget}
            pending={deleteMutation.isPending}
            errorMessage={deleteMutation.error?.message}
            onConfirm={() => deleteMutation.mutate({ id: deleteTarget.id })}
            onCancel={() => setDeleteTargetId(null)}
          />
        ) : rotateArmed && panelTarget !== undefined ? (
          <EndpointRotateConfirm
            pending={rotateMutation.isPending}
            onConfirm={() => rotateMutation.mutate({ endpointId: panelTarget.id })}
            onCancel={() => setRotateArmed(false)}
          />
        ) : (
          panelTarget !== undefined && (
            <EndpointMoreDetailBody
              endpoint={panelTarget}
              justCreatedSecret={justCreatedSecret}
              justRotatedSecret={justRotatedSecret}
              onRotate={() => setRotateArmed(true)}
              formId="endpoint-edit-form"
              form={form}
              eventTypes={eventTypes}
              onEventTypesChange={setEventTypes}
              onSubmit={saveEndpoint}
              saveErrorMessage={updateMutation.error?.message}
            />
          )
        )}
      </MoreDetailDrawer>

      {/* Create endpoint — short single-purpose form (§3). Not affected by
          the nested-Dialog-in-drawer bug: it opens from the toolbar while
          no drawer is open. */}
      <CreateEndpointDialog
        open={createOpen}
        onOpenChange={(open) => !open && setCreateOpen(false)}
        form={createForm}
        eventTypes={createEventTypes}
        onEventTypesChange={setCreateEventTypes}
        onSubmit={submitCreate}
        pending={createMutation.isPending}
        errorMessage={createMutation.error?.message}
      />

      <AttemptsToolbar
        refetchIntervalMs={attemptsRefetchIntervalMs}
        endpoints={endpointsQuery.data}
        endpointId={filters.endpointId}
        state={filters.state}
        onEndpointIdChange={(value) => void setFilters({ endpointId: value })}
        onStateChange={(value) => void setFilters({ state: value })}
        onClearFilters={() => void setFilters({ endpointId: null, state: null })}
        truncated={attemptsQuery.data?.truncated ?? false}
        errorMessage={attemptsQuery.error?.message}
      />

      <AttemptsTable
        attempts={attemptsQuery.data?.items}
        isLoading={attemptsQuery.isLoading}
        endpointUrlFor={endpointUrlFor}
        onRowClick={(a) => setQuickAttemptId(a.id)}
      />

      {/* Attempt quick detail — a peek, no route (D14). Body/footer swap to
          the inline replay confirmation when armed. */}
      <QuickDetailDrawer
        open={quickAttemptId !== null}
        onOpenChange={(open) => !open && setQuickAttemptId(null)}
        title={quickAttempt?.eventType ?? "Attempt"}
        description={
          quickAttempt !== undefined && <IdDisplay value={quickAttempt.id} variant="full" />
        }
        footer={
          replayArmed || quickAttempt === undefined ? undefined : (
            <AttemptQuickDetailFooter
              attempt={quickAttempt}
              replayPending={replayMutation.isPending}
              onClose={() => setQuickAttemptId(null)}
              onReplay={() => setReplayArmed(true)}
              onViewPayload={() => {
                void setAttemptPanelId(quickAttempt.id);
                setQuickAttemptId(null);
              }}
            />
          )
        }
      >
        {replayArmed && quickAttempt !== undefined ? (
          <AttemptReplayConfirm
            attempt={quickAttempt}
            endpointUrl={endpointUrlFor(quickAttempt.endpointId)}
            onConfirm={confirmReplay}
            onCancel={() => setReplayArmed(false)}
          />
        ) : (
          quickAttempt !== undefined && (
            <AttemptQuickDetailBody
              attempt={quickAttempt}
              endpointUrl={endpointUrlFor(quickAttempt.endpointId)}
            />
          )
        )}
      </QuickDetailDrawer>

      {/* Attempt more detail — the raw payload, owns `?attempt=<id>` (D14):
          a payload with per-exchange tabs is "its own internal structure",
          the reason §3 gives for upgrading past a narrow peek. */}
      <MoreDetailDrawer
        open={attemptPanelId !== null}
        onOpenChange={(open) => !open && void setAttemptPanelId(null)}
        title={attemptPanelTarget?.eventType ?? "Attempt payload"}
        description={
          attemptPanelTarget !== undefined && (
            <IdDisplay value={attemptPanelTarget.id} variant="full" />
          )
        }
        footer={
          <Button type="button" variant="ghost" onClick={() => void setAttemptPanelId(null)}>
            Close
          </Button>
        }
      >
        {attemptPanelTarget !== undefined && (
          <PayloadInspector
            exchanges={[
              {
                direction: "callback",
                method: "POST",
                url: endpointUrlFor(attemptPanelTarget.endpointId),
                body: payloadFor(attemptPanelTarget),
                ...(attemptPanelTarget.lastStatusCode !== undefined
                  ? { status: attemptPanelTarget.lastStatusCode }
                  : {}),
                ...(attemptPanelTarget.lastError !== undefined
                  ? { error: attemptPanelTarget.lastError }
                  : {}),
              },
            ]}
          />
        )}
      </MoreDetailDrawer>
    </ScreenStack>
  );
}
