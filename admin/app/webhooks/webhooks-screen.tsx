"use client";

// The Webhooks screen (#55): endpoint CRUD, attempt history with status
// codes and errors, one-click replay, and secret rotation with the overlap
// window made visible. "Endpoint CRUD, attempt history with status codes
// and errors, one-click replay, and rotation with the overlap window made
// visible" — the issue's own words, verbatim.
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
  PayloadInspector,
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
import { parseAsString, parseAsStringEnum, useQueryStates } from "nuqs";
import { useState } from "react";

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

interface EndpointFormState {
  url: string;
  eventTypes: EventType[];
  maskRecipient: boolean;
  active: boolean;
  maxAttempts: string;
}

export function WebhooksScreen() {
  const utils = trpc.useUtils();
  const endpointsQuery = trpc.webhookEndpoints.list.useQuery();

  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [form, setForm] = useState<EndpointFormState | null>(null);
  const [justCreatedSecret, setJustCreatedSecret] = useState<string | null>(null);
  const [justRotatedSecret, setJustRotatedSecret] = useState<string | null>(null);

  function openEndpoint(endpoint: EndpointListItem) {
    setSelectedId(endpoint.id);
    setForm({
      url: endpoint.url,
      eventTypes: endpoint.eventTypes,
      maskRecipient: endpoint.maskRecipient,
      active: endpoint.active,
      maxAttempts: String(endpoint.maxAttempts),
    });
    setJustCreatedSecret(null);
    setJustRotatedSecret(null);
  }

  const updateMutation = trpc.webhookEndpoints.update.useMutation({
    onSuccess: () => {
      toast({ title: "Endpoint saved", variant: "success" });
      void utils.webhookEndpoints.list.invalidate();
    },
  });

  const deleteMutation = trpc.webhookEndpoints.remove.useMutation({
    onSuccess: () => {
      toast({ title: "Endpoint deleted", variant: "success" });
      setSelectedId(null);
      setForm(null);
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
      if (selectedId === updated.id) {
        setForm({
          url: updated.url,
          eventTypes: updated.eventTypes,
          maskRecipient: updated.maskRecipient,
          active: updated.active,
          maxAttempts: String(updated.maxAttempts),
        });
      }
    },
  });

  const [createOpen, setCreateOpen] = useState(false);
  const [createForm, setCreateForm] = useState({
    appId: "",
    url: "",
    eventTypes: [] as EventType[],
    maskRecipient: true,
    maxAttempts: "8",
  });
  const createMutation = trpc.webhookEndpoints.create.useMutation({
    onSuccess: (created) => {
      toast({ title: "Endpoint created", variant: "success" });
      setCreateOpen(false);
      setCreateForm({ appId: "", url: "", eventTypes: [], maskRecipient: true, maxAttempts: "8" });
      void utils.webhookEndpoints.list.invalidate();
      openEndpoint({
        id: created.endpoint.id,
        appId: created.endpoint.appId,
        url: created.endpoint.url,
        eventTypes: created.endpoint.eventTypes,
        secret: created.endpoint.secret,
        prevSecret: created.endpoint.prevSecret,
        secretRotatedAt: created.endpoint.secretRotatedAt,
        maskRecipient: created.endpoint.maskRecipient,
        active: created.endpoint.active,
        maxAttempts: created.endpoint.maxAttempts,
        circuitOpenUntil: created.endpoint.circuitOpenUntil,
        consecutiveFailures: created.endpoint.consecutiveFailures,
        version: created.endpoint.version,
        createdAt: created.endpoint.createdAt,
        updatedAt: created.endpoint.updatedAt,
      });
      setJustCreatedSecret(created.secret);
    },
  });

  const selected = endpointsQuery.data?.find((e) => e.id === selectedId);

  function saveEndpoint() {
    if (selected === undefined || form === null) return;
    updateMutation.mutate({
      id: selected.id,
      etag: String(selected.version),
      url: form.url,
      eventTypes: form.eventTypes,
      maskRecipient: form.maskRecipient,
      active: form.active,
      maxAttempts: Number(form.maxAttempts),
    });
  }

  // --- Attempts history ---------------------------------------------------

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
  const [inspectTarget, setInspectTarget] = useState<AttemptListItem | null>(null);

  function endpointUrlFor(endpointId: string): string {
    return endpointsQuery.data?.find((e) => e.id === endpointId)?.url ?? endpointId;
  }

  function confirmReplay() {
    if (replayTarget === null) return;
    replayMutation.mutate({ attemptId: replayTarget.id });
    setReplayTarget(null);
  }

  return (
    <main className="mx-auto flex max-w-[1400px] flex-col gap-6 px-6 py-10">
      <header className="flex items-start justify-between gap-4 border-edge border-b pb-6">
        <div>
          <p className="font-mono text-micro text-subtle-foreground tracking-[0.03em]">
            vsms admin console
          </p>
          <h1 className="mt-1 font-medium text-foreground text-title">Webhooks</h1>
          <p className="mt-1 max-w-xl text-body text-muted-foreground">
            Endpoints, delivery attempts, and secret rotation.
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
            href="/sender-ids"
            className="text-caption text-muted-foreground underline decoration-edge-strong underline-offset-2 hover:decoration-foreground"
          >
            Sender IDs
          </a>
          <a
            href="/"
            className="text-caption text-muted-foreground underline decoration-edge-strong underline-offset-2 hover:decoration-foreground"
          >
            Composer
          </a>
        </div>
      </header>

      <div className="rounded-sm border border-edge bg-surface-2 px-3 py-2 text-caption text-muted-foreground">
        Reads and writes act as you — endpoint saves and secret rotation require{" "}
        <span className="font-mono text-foreground">webhook:manage</span> (owner, admin, and
        developer all carry it by default). The secret column below is the live value, not a
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
            <TableHead>Events</TableHead>
            <TableHead>Circuit</TableHead>
            <TableHead align="end">Updated</TableHead>
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
                onClick={() => openEndpoint(endpoint)}
              >
                <TableCell>
                  {endpoint.active ? (
                    <span className="text-state-success-fg">active</span>
                  ) : (
                    <span className="text-muted-foreground">inactive</span>
                  )}
                </TableCell>
                <TableCell mono>
                  <span className="line-clamp-1 max-w-[320px]" title={endpoint.url}>
                    {endpoint.url}
                  </span>
                </TableCell>
                <TableCell>
                  <span className="text-caption text-muted-foreground">
                    {endpoint.eventTypes.length} of {EVENT_TYPES.length}
                  </span>
                </TableCell>
                <TableCell>
                  {circuitOpen ? (
                    <span
                      className="text-state-danger-fg"
                      title={`Open until ${endpoint.circuitOpenUntil}`}
                    >
                      open ({endpoint.consecutiveFailures} failures)
                    </span>
                  ) : (
                    <span className="text-muted-foreground">closed</span>
                  )}
                </TableCell>
                <TableCell align="end">
                  <TimestampDisplay value={endpoint.updatedAt} />
                </TableCell>
              </TableRow>
            );
          })}
        </TableBody>
      </Table>

      {/* Endpoint detail / edit */}
      <Dialog
        open={selectedId !== null}
        onOpenChange={(open) => {
          if (!open) {
            setSelectedId(null);
            setForm(null);
          }
        }}
      >
        <DialogContent className="max-w-[640px]">
          <DialogHeader>
            <DialogTitle>{selected?.url ?? "Endpoint"}</DialogTitle>
            <DialogDescription>
              {selectedId !== null && <IdDisplay value={selectedId} variant="full" />}
            </DialogDescription>
          </DialogHeader>

          {selected != null && form != null && (
            <div className="flex flex-col gap-4">
              {justCreatedSecret != null && (
                <div className="rounded-sm border border-state-uncertain-border bg-state-uncertain-bg px-3 py-2 text-caption text-state-uncertain-fg">
                  This endpoint's secret is shown below — copy it into your receiver now. It stays
                  visible via "Reveal" any time afterward (see the screen's own note on why), but
                  this is the newest, safest moment to grab it.
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
                <SecretField label="Current secret" value={selected.secret} />
                {selected.prevSecret != null && (
                  <>
                    <SecretField
                      label="Previous secret (still verifies)"
                      value={selected.prevSecret}
                    />
                    <p className="text-caption text-subtle-foreground">
                      Rotated{" "}
                      {selected.secretRotatedAt != null && (
                        <TimestampDisplay value={selected.secretRotatedAt} />
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
                    onClick={() => setRotateTarget(selected)}
                  >
                    Rotate secret
                  </Button>
                </div>
              </div>

              <div className="flex flex-col gap-1.5">
                <Label htmlFor="endpoint-url">URL</Label>
                <Input
                  id="endpoint-url"
                  value={form.url}
                  onChange={(e) => setForm({ ...form, url: e.target.value })}
                />
              </div>

              <div className="flex flex-col gap-1.5">
                <Label>Event types</Label>
                <EventTypeToggles
                  selected={form.eventTypes}
                  onChange={(eventTypes) => setForm({ ...form, eventTypes })}
                />
              </div>

              <div className="grid grid-cols-2 gap-3">
                <div className="flex flex-col gap-1.5">
                  <Label htmlFor="endpoint-max-attempts">Max attempts</Label>
                  <Input
                    id="endpoint-max-attempts"
                    type="number"
                    min="1"
                    max="20"
                    value={form.maxAttempts}
                    onChange={(e) => setForm({ ...form, maxAttempts: e.target.value })}
                  />
                </div>
                <div className="flex flex-col justify-end gap-2 pb-2">
                  <label className="flex items-center gap-2 text-body text-foreground">
                    <input
                      type="checkbox"
                      checked={form.maskRecipient}
                      onChange={(e) => setForm({ ...form, maskRecipient: e.target.checked })}
                      className="checkbox"
                    />
                    Mask recipient MSISDN in payload
                  </label>
                  <label className="flex items-center gap-2 text-body text-foreground">
                    <input
                      type="checkbox"
                      checked={form.active}
                      onChange={(e) => setForm({ ...form, active: e.target.checked })}
                      className="checkbox"
                    />
                    Active
                  </label>
                </div>
              </div>

              {updateMutation.isError && (
                <div className="rounded-sm border border-state-danger-border bg-state-danger-bg px-3 py-2 text-caption text-state-danger-fg">
                  Save failed: {updateMutation.error.message}
                </div>
              )}
            </div>
          )}

          <DialogFooter className="justify-between">
            <Button
              type="button"
              variant="destructive"
              size="sm"
              onClick={() => selected != null && setDeleteTarget(selected)}
            >
              Delete
            </Button>
            <div className="flex gap-2">
              <Button
                type="button"
                variant="ghost"
                onClick={() => {
                  setSelectedId(null);
                  setForm(null);
                }}
              >
                Close
              </Button>
              <Button type="button" disabled={updateMutation.isPending} onClick={saveEndpoint}>
                {updateMutation.isPending ? "Saving…" : "Save"}
              </Button>
            </div>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* Create endpoint */}
      <Dialog open={createOpen} onOpenChange={(open) => !open && setCreateOpen(false)}>
        <DialogContent className="max-w-[560px]">
          <DialogHeader>
            <DialogTitle>New webhook endpoint</DialogTitle>
            <DialogDescription>
              A signing secret is generated automatically and shown once creation completes.
            </DialogDescription>
          </DialogHeader>
          <div className="flex flex-col gap-4">
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="new-endpoint-app-id">App ID</Label>
              <Input
                id="new-endpoint-app-id"
                placeholder="the App this endpoint belongs to"
                value={createForm.appId}
                onChange={(e) => setCreateForm({ ...createForm, appId: e.target.value })}
              />
            </div>
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="new-endpoint-url">URL</Label>
              <Input
                id="new-endpoint-url"
                placeholder="https://example.com/webhooks/vsms"
                value={createForm.url}
                onChange={(e) => setCreateForm({ ...createForm, url: e.target.value })}
              />
            </div>
            <div className="flex flex-col gap-1.5">
              <Label>Event types</Label>
              <EventTypeToggles
                selected={createForm.eventTypes}
                onChange={(eventTypes) => setCreateForm({ ...createForm, eventTypes })}
              />
            </div>
            <div className="grid grid-cols-2 gap-3">
              <div className="flex flex-col gap-1.5">
                <Label htmlFor="new-endpoint-max-attempts">Max attempts</Label>
                <Input
                  id="new-endpoint-max-attempts"
                  type="number"
                  min="1"
                  max="20"
                  value={createForm.maxAttempts}
                  onChange={(e) => setCreateForm({ ...createForm, maxAttempts: e.target.value })}
                />
              </div>
              <div className="flex flex-col justify-end pb-2">
                <label className="flex items-center gap-2 text-body text-foreground">
                  <input
                    type="checkbox"
                    checked={createForm.maskRecipient}
                    onChange={(e) =>
                      setCreateForm({ ...createForm, maskRecipient: e.target.checked })
                    }
                    className="checkbox"
                  />
                  Mask recipient MSISDN
                </label>
              </div>
            </div>
            {createMutation.isError && (
              <div className="rounded-sm border border-state-danger-border bg-state-danger-bg px-3 py-2 text-caption text-state-danger-fg">
                Create failed: {createMutation.error.message}
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
                createMutation.isPending ||
                createForm.appId === "" ||
                createForm.url === "" ||
                createForm.eventTypes.length === 0
              }
              onClick={() =>
                createMutation.mutate({
                  appId: createForm.appId,
                  url: createForm.url,
                  eventTypes: createForm.eventTypes,
                  maskRecipient: createForm.maskRecipient,
                  maxAttempts: Number(createForm.maxAttempts),
                })
              }
            >
              {createMutation.isPending ? "Creating…" : "Create"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* Delete confirm */}
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

      {/* Rotate secret confirm */}
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
            <SelectTrigger id="attempts-endpoint" className="w-[280px]">
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
            <SelectTrigger id="attempts-state" className="w-[200px]">
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
            <TableHead>Event</TableHead>
            <TableHead>Endpoint</TableHead>
            <TableHead>Attempts</TableHead>
            <TableHead>Status</TableHead>
            <TableHead>Last error</TableHead>
            <TableHead>Last attempt</TableHead>
            <TableHead align="end">Action</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {attemptsQuery.isLoading &&
            Array.from({ length: 6 }).map((_, i) => (
              // biome-ignore lint/suspicious/noArrayIndexKey: static skeleton rows, never reordered or diffed
              <TableRow key={i}>
                <TableCell colSpan={8}>
                  <Skeleton className="h-4 w-full" />
                </TableCell>
              </TableRow>
            ))}

          {!attemptsQuery.isLoading && (attemptsQuery.data?.items.length ?? 0) === 0 && (
            <tr>
              <td colSpan={8}>
                <InlineEmptyState message="No delivery attempts match the current filters." />
              </td>
            </tr>
          )}

          {attemptsQuery.data?.items.map((attempt) => (
            <TableRow
              key={attempt.id}
              className="cursor-pointer"
              onClick={() => setInspectTarget(attempt)}
            >
              <TableCell>
                <AttemptStatusPill state={attempt.state} />
              </TableCell>
              <TableCell mono>{attempt.eventType}</TableCell>
              <TableCell mono>
                <span
                  className="line-clamp-1 max-w-[220px]"
                  title={endpointUrlFor(attempt.endpointId)}
                >
                  {endpointUrlFor(attempt.endpointId)}
                </span>
              </TableCell>
              <TableCell mono>{attempt.attempts}</TableCell>
              <TableCell mono>{attempt.lastStatusCode ?? "—"}</TableCell>
              <TableCell>
                {attempt.lastError != null ? (
                  <span
                    className="line-clamp-1 max-w-[260px] text-caption text-muted-foreground"
                    title={attempt.lastError}
                  >
                    {attempt.lastError}
                  </span>
                ) : (
                  <span className="text-muted-foreground">—</span>
                )}
              </TableCell>
              <TableCell>
                {attempt.lastAttemptAt != null ? (
                  <TimestampDisplay value={attempt.lastAttemptAt} />
                ) : (
                  <span className="text-muted-foreground">never</span>
                )}
              </TableCell>
              <TableCell align="end">
                {(attempt.state === "failed" || attempt.state === "dead") && (
                  <Button
                    type="button"
                    variant="secondary"
                    size="sm"
                    disabled={replayMutation.isPending}
                    onClick={(e) => {
                      e.stopPropagation();
                      setReplayTarget(attempt);
                    }}
                  >
                    Replay
                  </Button>
                )}
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>

      {/* Payload inspector */}
      <Dialog
        open={inspectTarget !== null}
        onOpenChange={(open) => !open && setInspectTarget(null)}
      >
        <DialogContent className="max-w-[640px]">
          <DialogHeader>
            <DialogTitle>{inspectTarget?.eventType}</DialogTitle>
            <DialogDescription>
              {inspectTarget != null && <IdDisplay value={inspectTarget.id} variant="full" />}
            </DialogDescription>
          </DialogHeader>
          {inspectTarget != null && (
            <PayloadInspector
              exchanges={[
                {
                  direction: "callback",
                  method: "POST",
                  url: endpointUrlFor(inspectTarget.endpointId),
                  body: (() => {
                    try {
                      return JSON.stringify(JSON.parse(inspectTarget.payload), null, 2);
                    } catch {
                      return inspectTarget.payload;
                    }
                  })(),
                  ...(inspectTarget.lastStatusCode !== undefined
                    ? { status: inspectTarget.lastStatusCode }
                    : {}),
                  ...(inspectTarget.lastError !== undefined
                    ? { error: inspectTarget.lastError }
                    : {}),
                },
              ]}
            />
          )}
        </DialogContent>
      </Dialog>

      {/* Replay confirm */}
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
