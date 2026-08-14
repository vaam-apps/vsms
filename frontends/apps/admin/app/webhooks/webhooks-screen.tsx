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
// type on this same screen. Delete and secret rotation are irreversible-
// enough to stay centered `Dialog`s (§1.7), opened from inside the
// endpoint's more-detail drawer.
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
// `rotateWebhookSecret`). The confirm dialog below says this before the
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

import { zodResolver } from "@hookform/resolvers/zod";
import type { inferRouterOutputs } from "@trpc/server";
import type { AppRouter } from "@vsms/api";
import { trpc } from "@vsms/hooks";
import {
  ATTEMPT_STATES,
  type AttemptState,
  AttemptStatusPill,
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
  PayloadInspector,
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
import { parseAsString, parseAsStringEnum, useQueryState, useQueryStates } from "nuqs";
import { useState } from "react";
import { Controller, useForm } from "react-hook-form";
import { z } from "zod";

type RouterOutputs = inferRouterOutputs<AppRouter>;
type EndpointListItem = RouterOutputs["webhookEndpoints"]["list"][number];
type AttemptListItem = RouterOutputs["webhookAttempts"]["list"]["items"][number];

const EVENT_TYPES = [
  "message.accepted",
  "message.submitted",
  "message.delivered",
  "message.failed",
  "message.expired",
  "message.uncertain",
  "message.cancelled",
] as const;
type EventType = (typeof EVENT_TYPES)[number];

const REFETCH_INTERVAL_MS = 5000;

function maskSecret(value: string): string {
  const tail = value.length > 4 ? value.slice(-4) : value;
  return `whsec_${"•".repeat(10)}${tail}`;
}

function SecretField({ label, value }: { label: string; value: string }) {
  const [revealed, setRevealed] = useState(false);
  const [copied, setCopied] = useState(false);

  async function copy() {
    await navigator.clipboard.writeText(value);
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  }

  return (
    <div className="flex flex-col gap-1">
      <p className="text-caption text-subtle-foreground">{label}</p>
      <div className="flex items-center gap-2">
        <code className="flex-1 truncate rounded-sm border border-edge bg-surface-2 px-2 py-1 font-mono text-caption text-foreground">
          {revealed ? value : maskSecret(value)}
        </code>
        <Button type="button" variant="ghost" size="sm" onClick={() => setRevealed(!revealed)}>
          {revealed ? "Hide" : "Reveal"}
        </Button>
        <Button type="button" variant="ghost" size="sm" onClick={copy}>
          {copied ? "Copied" : "Copy"}
        </Button>
      </div>
    </div>
  );
}

function EventTypeToggles({
  selected,
  onChange,
}: {
  selected: EventType[];
  onChange: (types: EventType[]) => void;
}) {
  function toggle(type: EventType) {
    onChange(selected.includes(type) ? selected.filter((t) => t !== type) : [...selected, type]);
  }
  return (
    <div className="flex flex-wrap gap-1.5">
      {EVENT_TYPES.map((type) => {
        const active = selected.includes(type);
        return (
          <button
            key={type}
            type="button"
            onClick={() => toggle(type)}
            aria-pressed={active}
            className={
              active
                ? "rounded-sm border border-foreground bg-foreground px-2 py-1 font-mono text-background text-caption"
                : "rounded-sm border border-edge px-2 py-1 font-mono text-caption text-muted-foreground hover:border-edge-strong"
            }
          >
            {type}
          </button>
        );
      })}
    </div>
  );
}

const endpointSchema = z.object({
  url: z.string().trim().min(1, "URL is required"),
  maxAttempts: z
    .string()
    .trim()
    .refine((v) => Number.isInteger(Number(v)) && Number(v) >= 1 && Number(v) <= 20, {
      message: "1–20",
    }),
  maskRecipient: z.boolean(),
  active: z.boolean(),
});
type EndpointFormValues = z.infer<typeof endpointSchema>;

const createSchema = z.object({
  appId: z.string().trim().min(1, "App id is required"),
  url: z.string().trim().min(1, "URL is required"),
  maxAttempts: z
    .string()
    .trim()
    .refine((v) => Number.isInteger(Number(v)) && Number(v) >= 1 && Number(v) <= 20, {
      message: "1–20",
    }),
  maskRecipient: z.boolean(),
});
type CreateFormValues = z.infer<typeof createSchema>;

export function WebhooksScreen() {
  const utils = trpc.useUtils();
  const endpointsQuery = trpc.webhookEndpoints.list.useQuery();

  // --- Endpoints: quick + more detail ---------------------------------

  const [quickId, setQuickId] = useState<string | null>(null);
  const quickDetail = endpointsQuery.data?.find((e) => e.id === quickId);

  const [panelId, setPanelId] = useQueryState("panel", parseAsString);
  const panelTarget = endpointsQuery.data?.find((e) => e.id === panelId);

  const [eventTypes, setEventTypes] = useState<EventType[]>([]);
  const form = useForm<EndpointFormValues>({ resolver: zodResolver(endpointSchema) });

  function openMore(endpoint: EndpointListItem) {
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
  }

  const deleteMutation = trpc.webhookEndpoints.remove.useMutation({
    onSuccess: () => {
      toast({ title: "Endpoint deleted", variant: "success" });
      setDeleteTarget(null);
      closeMore();
      void utils.webhookEndpoints.list.invalidate();
      void utils.webhookAttempts.list.invalidate();
    },
  });
  const [deleteTarget, setDeleteTarget] = useState<EndpointListItem | null>(null);

  const [rotateTarget, setRotateTarget] = useState<EndpointListItem | null>(null);
  const rotateMutation = trpc.webhookEndpoints.rotateSecret.useMutation({
    onSuccess: (updated) => {
      toast({ title: "Secret rotated", variant: "success" });
      setRotateTarget(null);
      setJustRotatedSecret(updated.secret);
      void utils.webhookEndpoints.list.invalidate();
    },
  });

  const [createOpen, setCreateOpen] = useState(false);
  const [createEventTypes, setCreateEventTypes] = useState<EventType[]>([]);
  const createForm = useForm<CreateFormValues>({
    resolver: zodResolver(createSchema),
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

  function submitCreate(values: CreateFormValues) {
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
    { refetchInterval: REFETCH_INTERVAL_MS },
  );

  const replayMutation = trpc.webhookAttempts.replay.useMutation({
    onSuccess: () => {
      void utils.webhookAttempts.list.invalidate();
    },
  });
  const [replayTarget, setReplayTarget] = useState<AttemptListItem | null>(null);
  const [quickAttemptId, setQuickAttemptId] = useState<string | null>(null);
  const quickAttempt = attemptsQuery.data?.items.find((a) => a.id === quickAttemptId);
  const [attemptPanelId, setAttemptPanelId] = useQueryState("attempt", parseAsString);
  const attemptPanelTarget = attemptsQuery.data?.items.find((a) => a.id === attemptPanelId);

  function endpointUrlFor(endpointId: string): string {
    return endpointsQuery.data?.find((e) => e.id === endpointId)?.url ?? endpointId;
  }

  function confirmReplay() {
    if (replayTarget === null) return;
    replayMutation.mutate({ attemptId: replayTarget.id });
    setReplayTarget(null);
  }

  function payloadFor(attempt: AttemptListItem) {
    try {
      return JSON.stringify(JSON.parse(attempt.payload), null, 2);
    } catch {
      return attempt.payload;
    }
  }

  return (
    <main className="mx-auto flex max-w-[1400px] flex-col gap-6 px-4 py-6 sm:px-6 sm:py-10">
      <header className="flex flex-col gap-1 border-edge border-b pb-6">
        <p className="font-mono text-micro text-subtle-foreground tracking-[0.03em]">
          vsms admin console
        </p>
        <h1 className="font-medium text-foreground text-title">Webhooks</h1>
        <p className="max-w-xl text-body text-muted-foreground">
          Endpoints, delivery attempts, and secret rotation.
        </p>
      </header>

      <div className="rounded-sm border border-edge bg-surface-2 px-3 py-2 text-caption text-muted-foreground">
        Reads and writes act as you — endpoint saves and secret rotation require{" "}
        <span className="font-mono text-foreground">webhook:manage</span> (owner, admin, and
        developer all carry it by default). The secret shown below is the live value, not a
        placeholder — masked here as a screen-share precaution, not a security boundary; see the
        screen's own note for why.
      </div>

      <div className="flex items-center justify-between">
        <h2 className="font-medium text-body text-foreground">Endpoints</h2>
        <Button type="button" size="sm" onClick={() => setCreateOpen(true)}>
          New endpoint
        </Button>
      </div>

      {endpointsQuery.isError && (
        <div className="rounded-sm border border-state-danger-border bg-state-danger-bg px-3 py-2 text-caption text-state-danger-fg">
          Could not read webhook endpoints: {endpointsQuery.error.message}
        </div>
      )}

      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>Active</TableHead>
            <TableHead>URL</TableHead>
            <TableHead className="hidden md:table-cell">Events</TableHead>
            <TableHead className="hidden sm:table-cell">Circuit</TableHead>
            <TableHead align="end" className="hidden md:table-cell">
              Updated
            </TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {endpointsQuery.isLoading &&
            Array.from({ length: 3 }).map((_, i) => (
              // biome-ignore lint/suspicious/noArrayIndexKey: static skeleton rows, never reordered or diffed
              <TableRow key={i}>
                <TableCell colSpan={5}>
                  <Skeleton className="h-4 w-full" />
                </TableCell>
              </TableRow>
            ))}

          {!endpointsQuery.isLoading && (endpointsQuery.data?.length ?? 0) === 0 && (
            <tr>
              <td colSpan={5}>
                <InlineEmptyState message="No webhook endpoints configured yet." />
              </td>
            </tr>
          )}

          {endpointsQuery.data?.map((endpoint) => {
            const circuitOpen =
              endpoint.circuitOpenUntil != null && new Date(endpoint.circuitOpenUntil) > new Date();
            return (
              <TableRow
                key={endpoint.id}
                className="cursor-pointer"
                onClick={() => setQuickId(endpoint.id)}
              >
                <TableCell>
                  {endpoint.active ? (
                    <span className="text-state-success-fg">active</span>
                  ) : (
                    <span className="text-muted-foreground">inactive</span>
                  )}
                </TableCell>
                <TableCell mono>
                  <span
                    className="line-clamp-1 max-w-[220px] sm:max-w-[320px]"
                    title={endpoint.url}
                  >
                    {endpoint.url}
                  </span>
                </TableCell>
                <TableCell className="hidden md:table-cell">
                  <span className="text-caption text-muted-foreground">
                    {endpoint.eventTypes.length} of {EVENT_TYPES.length}
                  </span>
                </TableCell>
                <TableCell className="hidden sm:table-cell">
                  {circuitOpen ? (
                    <span
                      className="text-state-danger-fg"
                      title={`Open until ${endpoint.circuitOpenUntil}`}
                    >
                      open ({endpoint.consecutiveFailures})
                    </span>
                  ) : (
                    <span className="text-muted-foreground">closed</span>
                  )}
                </TableCell>
                <TableCell align="end" className="hidden md:table-cell">
                  <TimestampDisplay value={endpoint.updatedAt} />
                </TableCell>
              </TableRow>
            );
          })}
        </TableBody>
      </Table>

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
        {quickDetail !== undefined && (
          <dl className="flex flex-col gap-3 text-body">
            <div className="flex items-center justify-between gap-3">
              <dt className="text-muted-foreground">Active</dt>
              <dd>{quickDetail.active ? "yes" : "no"}</dd>
            </div>
            <div className="flex items-center justify-between gap-3">
              <dt className="text-muted-foreground">Events</dt>
              <dd className="font-mono text-caption">
                {quickDetail.eventTypes.length} of {EVENT_TYPES.length}
              </dd>
            </div>
            <div className="flex items-center justify-between gap-3">
              <dt className="text-muted-foreground">Circuit</dt>
              <dd>
                {quickDetail.circuitOpenUntil != null &&
                new Date(quickDetail.circuitOpenUntil) > new Date() ? (
                  <span className="text-state-danger-fg">
                    open ({quickDetail.consecutiveFailures} failures)
                  </span>
                ) : (
                  <span className="text-muted-foreground">closed</span>
                )}
              </dd>
            </div>
            <SecretField label="Current secret" value={quickDetail.secret} />
          </dl>
        )}
      </QuickDetailDrawer>

      {/* More detail — the endpoint's own edit form, secret reveal/rotate,
          and Delete (D14). */}
      <MoreDetailDrawer
        open={panelId !== null}
        onOpenChange={(open) => !open && closeMore()}
        title={panelTarget?.url ?? "Endpoint"}
        description={
          panelTarget !== undefined && <IdDisplay value={panelTarget.id} variant="full" />
        }
        footer={
          <>
            {panelTarget !== undefined && (
              <Button
                type="button"
                variant="destructive"
                size="sm"
                className="mr-auto"
                onClick={() => setDeleteTarget(panelTarget)}
              >
                Delete
              </Button>
            )}
            <Button type="button" variant="ghost" onClick={closeMore}>
              Close
            </Button>
            <Button type="submit" form="endpoint-edit-form" disabled={updateMutation.isPending}>
              {updateMutation.isPending ? "Saving…" : "Save"}
            </Button>
          </>
        }
      >
        {panelTarget !== undefined && (
          <div className="flex flex-col gap-4">
            {justCreatedSecret != null && (
              <div className="rounded-sm border border-state-uncertain-border bg-state-uncertain-bg px-3 py-2 text-caption text-state-uncertain-fg">
                This endpoint's secret is shown below — copy it into your receiver now. It stays
                visible via "Reveal" any time afterward (see the screen's own note on why), but this
                is the newest, safest moment to grab it.
              </div>
            )}
            {justRotatedSecret != null && (
              <div className="rounded-sm border border-state-uncertain-border bg-state-uncertain-bg px-3 py-2 text-caption text-state-uncertain-fg">
                Rotated. The new secret is below — copy it into your receiver. Your{" "}
                <span className="font-mono">previous secret</span> keeps verifying until the{" "}
                <em>next</em> rotation, so there is no rush, but don't wait indefinitely.
              </div>
            )}

            <div className="flex flex-col gap-3 rounded-sm border border-edge bg-surface-2 p-3">
              <SecretField label="Current secret" value={panelTarget.secret} />
              {panelTarget.prevSecret != null && (
                <>
                  <SecretField
                    label="Previous secret (still verifies)"
                    value={panelTarget.prevSecret}
                  />
                  <p className="text-caption text-subtle-foreground">
                    Rotated{" "}
                    {panelTarget.secretRotatedAt != null && (
                      <TimestampDisplay value={panelTarget.secretRotatedAt} />
                    )}{" "}
                    — this value keeps accepting signatures until you rotate again. There is no
                    fixed expiry.
                  </p>
                </>
              )}
              <div>
                <Button
                  type="button"
                  variant="secondary"
                  size="sm"
                  onClick={() => setRotateTarget(panelTarget)}
                >
                  Rotate secret
                </Button>
              </div>
            </div>

            <form
              id="endpoint-edit-form"
              onSubmit={form.handleSubmit(saveEndpoint)}
              className="flex flex-col gap-4"
            >
              <div className="flex flex-col gap-1.5">
                <Label htmlFor="endpoint-url">URL</Label>
                <Input
                  id="endpoint-url"
                  aria-invalid={form.formState.errors.url != null}
                  {...form.register("url")}
                />
                {form.formState.errors.url != null && (
                  <p className="text-caption text-state-danger-fg">
                    {form.formState.errors.url.message}
                  </p>
                )}
              </div>

              <div className="flex flex-col gap-1.5">
                <Label>Event types</Label>
                <EventTypeToggles selected={eventTypes} onChange={setEventTypes} />
              </div>

              <div className="grid grid-cols-2 gap-3">
                <div className="flex flex-col gap-1.5">
                  <Label htmlFor="endpoint-max-attempts">Max attempts</Label>
                  <Input
                    id="endpoint-max-attempts"
                    inputMode="numeric"
                    aria-invalid={form.formState.errors.maxAttempts != null}
                    {...form.register("maxAttempts")}
                  />
                  {form.formState.errors.maxAttempts != null && (
                    <p className="text-caption text-state-danger-fg">
                      {form.formState.errors.maxAttempts.message}
                    </p>
                  )}
                </div>
                <div className="flex flex-col justify-end gap-2 pb-2">
                  <Controller
                    control={form.control}
                    name="maskRecipient"
                    render={({ field }) => (
                      <label className="flex items-center gap-2 text-body text-foreground">
                        <input
                          type="checkbox"
                          checked={field.value}
                          onChange={(e) => field.onChange(e.target.checked)}
                          className="checkbox"
                        />
                        Mask recipient MSISDN in payload
                      </label>
                    )}
                  />
                  <Controller
                    control={form.control}
                    name="active"
                    render={({ field }) => (
                      <label className="flex items-center gap-2 text-body text-foreground">
                        <input
                          type="checkbox"
                          checked={field.value}
                          onChange={(e) => field.onChange(e.target.checked)}
                          className="checkbox"
                        />
                        Active
                      </label>
                    )}
                  />
                </div>
              </div>

              {updateMutation.isError && (
                <div className="rounded-sm border border-state-danger-border bg-state-danger-bg px-3 py-2 text-caption text-state-danger-fg">
                  Save failed: {updateMutation.error.message}
                </div>
              )}
            </form>
          </div>
        )}
      </MoreDetailDrawer>

      {/* Create endpoint — short single-purpose form (§3). */}
      <Dialog open={createOpen} onOpenChange={(open) => !open && setCreateOpen(false)}>
        <DialogContent className="max-w-[560px]">
          <DialogHeader>
            <DialogTitle>New webhook endpoint</DialogTitle>
            <DialogDescription>
              A signing secret is generated automatically and shown once creation completes.
            </DialogDescription>
          </DialogHeader>
          <form
            id="create-endpoint-form"
            onSubmit={createForm.handleSubmit(submitCreate)}
            className="flex flex-col gap-4"
          >
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="new-endpoint-app-id">App ID</Label>
              <Input
                id="new-endpoint-app-id"
                placeholder="the App this endpoint belongs to"
                aria-invalid={createForm.formState.errors.appId != null}
                {...createForm.register("appId")}
              />
            </div>
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="new-endpoint-url">URL</Label>
              <Input
                id="new-endpoint-url"
                placeholder="https://example.com/webhooks/vsms"
                aria-invalid={createForm.formState.errors.url != null}
                {...createForm.register("url")}
              />
            </div>
            <div className="flex flex-col gap-1.5">
              <Label>Event types</Label>
              <EventTypeToggles selected={createEventTypes} onChange={setCreateEventTypes} />
            </div>
            <div className="grid grid-cols-2 gap-3">
              <div className="flex flex-col gap-1.5">
                <Label htmlFor="new-endpoint-max-attempts">Max attempts</Label>
                <Input
                  id="new-endpoint-max-attempts"
                  inputMode="numeric"
                  {...createForm.register("maxAttempts")}
                />
              </div>
              <div className="flex flex-col justify-end pb-2">
                <Controller
                  control={createForm.control}
                  name="maskRecipient"
                  render={({ field }) => (
                    <label className="flex items-center gap-2 text-body text-foreground">
                      <input
                        type="checkbox"
                        checked={field.value}
                        onChange={(e) => field.onChange(e.target.checked)}
                        className="checkbox"
                      />
                      Mask recipient MSISDN
                    </label>
                  )}
                />
              </div>
            </div>
            {createMutation.isError && (
              <div className="rounded-sm border border-state-danger-border bg-state-danger-bg px-3 py-2 text-caption text-state-danger-fg">
                Create failed: {createMutation.error.message}
              </div>
            )}
          </form>
          <DialogFooter>
            <Button type="button" variant="ghost" onClick={() => setCreateOpen(false)}>
              Cancel
            </Button>
            <Button
              type="submit"
              form="create-endpoint-form"
              disabled={createMutation.isPending || createEventTypes.length === 0}
            >
              {createMutation.isPending ? "Creating…" : "Create"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* Delete confirm — destructive, centered Dialog (§1.7/§3). */}
      <Dialog open={deleteTarget !== null} onOpenChange={(open) => !open && setDeleteTarget(null)}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Delete this endpoint?</DialogTitle>
            <DialogDescription>
              {deleteTarget != null && (
                <>
                  Stops all future deliveries to{" "}
                  <span className="font-mono text-foreground">{deleteTarget.url}</span>. Attempts
                  already recorded against it are not deleted.
                </>
              )}
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button type="button" variant="ghost" onClick={() => setDeleteTarget(null)}>
              Cancel
            </Button>
            <Button
              type="button"
              variant="destructive"
              disabled={deleteMutation.isPending}
              onClick={() => deleteTarget != null && deleteMutation.mutate({ id: deleteTarget.id })}
            >
              {deleteMutation.isPending ? "Deleting…" : "Delete"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* Rotate secret confirm — destructive-with-consequence, centered
          Dialog (§1.7/§3). */}
      <Dialog open={rotateTarget !== null} onOpenChange={(open) => !open && setRotateTarget(null)}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Rotate this endpoint's secret?</DialogTitle>
            <DialogDescription>
              A new secret is minted immediately and every future delivery is signed with it. The{" "}
              <strong>current</strong> secret keeps verifying as the "previous secret" — not for a
              fixed time, but until you rotate <em>again</em>. If your receiver hasn't been updated
              to accept the new value before that happens, its signature checks will start failing
              at that point, not now.
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button type="button" variant="ghost" onClick={() => setRotateTarget(null)}>
              Cancel
            </Button>
            <Button
              type="button"
              disabled={rotateMutation.isPending}
              onClick={() =>
                rotateTarget != null && rotateMutation.mutate({ endpointId: rotateTarget.id })
              }
            >
              {rotateMutation.isPending ? "Rotating…" : "Rotate secret"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <div className="mt-4 flex items-center justify-between border-edge border-t pt-6">
        <h2 className="font-medium text-body text-foreground">Delivery attempts</h2>
        <p className="text-caption text-subtle-foreground">
          Refreshes every {Math.round(REFETCH_INTERVAL_MS / 1000)}s
        </p>
      </div>

      <div className="flex flex-wrap items-end gap-4">
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="attempts-endpoint">Endpoint</Label>
          <Select
            value={filters.endpointId ?? "__all"}
            onValueChange={(value) =>
              void setFilters({ endpointId: value === "__all" ? null : value })
            }
          >
            <SelectTrigger id="attempts-endpoint" className="w-[220px] sm:w-[280px]">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="__all">All endpoints</SelectItem>
              {endpointsQuery.data?.map((endpoint) => (
                <SelectItem key={endpoint.id} value={endpoint.id}>
                  {endpoint.url}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="attempts-state">State</Label>
          <Select
            value={filters.state ?? "__all"}
            onValueChange={(value) =>
              void setFilters({ state: value === "__all" ? null : (value as AttemptState) })
            }
          >
            <SelectTrigger id="attempts-state" className="w-[160px] sm:w-[200px]">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="__all">All states</SelectItem>
              {ATTEMPT_STATES.map((state) => (
                <SelectItem key={state} value={state}>
                  {state}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
        {(filters.endpointId !== null || filters.state !== null) && (
          <Button
            type="button"
            variant="ghost"
            size="sm"
            onClick={() => void setFilters({ endpointId: null, state: null })}
          >
            Clear filters
          </Button>
        )}
      </div>

      {attemptsQuery.data?.truncated && (
        <p className="text-caption text-subtle-foreground">
          Showing the most recent 1000 attempts — filtering happens over that window.
        </p>
      )}

      {attemptsQuery.isError && (
        <div className="rounded-sm border border-state-danger-border bg-state-danger-bg px-3 py-2 text-caption text-state-danger-fg">
          Could not read attempts: {attemptsQuery.error.message}
        </div>
      )}

      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>State</TableHead>
            <TableHead className="hidden md:table-cell">Event</TableHead>
            <TableHead className="hidden lg:table-cell">Endpoint</TableHead>
            <TableHead align="end" className="hidden sm:table-cell">
              Attempts
            </TableHead>
            <TableHead className="hidden sm:table-cell">Status</TableHead>
            <TableHead className="hidden md:table-cell">Last attempt</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {attemptsQuery.isLoading &&
            Array.from({ length: 6 }).map((_, i) => (
              // biome-ignore lint/suspicious/noArrayIndexKey: static skeleton rows, never reordered or diffed
              <TableRow key={i}>
                <TableCell colSpan={6}>
                  <Skeleton className="h-4 w-full" />
                </TableCell>
              </TableRow>
            ))}

          {!attemptsQuery.isLoading && (attemptsQuery.data?.items.length ?? 0) === 0 && (
            <tr>
              <td colSpan={6}>
                <InlineEmptyState message="No delivery attempts match the current filters." />
              </td>
            </tr>
          )}

          {attemptsQuery.data?.items.map((attempt) => (
            <TableRow
              key={attempt.id}
              className="cursor-pointer"
              onClick={() => setQuickAttemptId(attempt.id)}
            >
              <TableCell>
                <AttemptStatusPill state={attempt.state} />
              </TableCell>
              <TableCell mono className="hidden md:table-cell">
                {attempt.eventType}
              </TableCell>
              <TableCell mono className="hidden lg:table-cell">
                <span
                  className="line-clamp-1 max-w-[220px]"
                  title={endpointUrlFor(attempt.endpointId)}
                >
                  {endpointUrlFor(attempt.endpointId)}
                </span>
              </TableCell>
              <TableCell mono align="end" className="hidden sm:table-cell">
                {attempt.attempts}
              </TableCell>
              <TableCell mono className="hidden sm:table-cell">
                {attempt.lastStatusCode ?? "—"}
              </TableCell>
              <TableCell className="hidden md:table-cell">
                {attempt.lastAttemptAt != null ? (
                  <TimestampDisplay value={attempt.lastAttemptAt} />
                ) : (
                  <span className="text-muted-foreground">never</span>
                )}
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>

      {/* Attempt quick detail — a peek, no route (D14). */}
      <QuickDetailDrawer
        open={quickAttemptId !== null}
        onOpenChange={(open) => !open && setQuickAttemptId(null)}
        title={quickAttempt?.eventType ?? "Attempt"}
        description={
          quickAttempt !== undefined && <IdDisplay value={quickAttempt.id} variant="full" />
        }
        footer={
          <>
            <Button type="button" variant="ghost" size="sm" onClick={() => setQuickAttemptId(null)}>
              Close
            </Button>
            {quickAttempt !== undefined &&
              (quickAttempt.state === "failed" || quickAttempt.state === "dead") && (
                <Button
                  type="button"
                  variant="secondary"
                  size="sm"
                  disabled={replayMutation.isPending}
                  onClick={() => setReplayTarget(quickAttempt)}
                >
                  Replay
                </Button>
              )}
            <Button
              type="button"
              size="sm"
              onClick={() => {
                if (quickAttempt === undefined) return;
                void setAttemptPanelId(quickAttempt.id);
                setQuickAttemptId(null);
              }}
            >
              View payload
            </Button>
          </>
        }
      >
        {quickAttempt !== undefined && (
          <dl className="flex flex-col gap-3 text-body">
            <div className="flex items-center justify-between gap-3">
              <dt className="text-muted-foreground">State</dt>
              <dd>
                <AttemptStatusPill state={quickAttempt.state} showLiteral />
              </dd>
            </div>
            <div className="flex items-center justify-between gap-3">
              <dt className="text-muted-foreground">Endpoint</dt>
              <dd className="max-w-[240px] truncate font-mono text-caption">
                {endpointUrlFor(quickAttempt.endpointId)}
              </dd>
            </div>
            <div className="flex items-center justify-between gap-3">
              <dt className="text-muted-foreground">Attempts</dt>
              <dd className="font-mono">{quickAttempt.attempts}</dd>
            </div>
            <div className="flex items-center justify-between gap-3">
              <dt className="text-muted-foreground">Last status code</dt>
              <dd className="font-mono">{quickAttempt.lastStatusCode ?? "—"}</dd>
            </div>
            {quickAttempt.lastError != null && (
              <div className="flex flex-col gap-1">
                <dt className="text-muted-foreground">Last error</dt>
                <dd className="text-caption text-state-danger-fg">{quickAttempt.lastError}</dd>
              </div>
            )}
            <div className="flex items-center justify-between gap-3">
              <dt className="text-muted-foreground">Last attempt</dt>
              <dd>
                {quickAttempt.lastAttemptAt != null ? (
                  <TimestampDisplay value={quickAttempt.lastAttemptAt} />
                ) : (
                  <span className="text-muted-foreground">never</span>
                )}
              </dd>
            </div>
          </dl>
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

      {/* Replay confirm. */}
      <Dialog open={replayTarget !== null} onOpenChange={(open) => !open && setReplayTarget(null)}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Replay this delivery?</DialogTitle>
            <DialogDescription>
              {replayTarget != null && (
                <>
                  Re-fires exactly one attempt of{" "}
                  <span className="font-mono text-foreground">{replayTarget.eventType}</span> to{" "}
                  <span className="font-mono text-foreground">
                    {endpointUrlFor(replayTarget.endpointId)}
                  </span>
                  , using the payload captured when the event first fired — not a fresh copy of the
                  message's current state, and possibly old. Also clears the endpoint's circuit
                  breaker if it was open.
                </>
              )}
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button type="button" variant="ghost" onClick={() => setReplayTarget(null)}>
              Cancel
            </Button>
            <Button type="button" onClick={confirmReplay}>
              Replay
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </main>
  );
}
