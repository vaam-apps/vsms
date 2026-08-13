"use client";

// The Apps screen (#52): apps, their service-account clients, quota, and
// the ipAllowlist/transliterateToGsm7 toggles — plus #211's own real
// reads-and-writes-as-you proof case, the same shape `providers-screen.tsx`
// already established. `App.update`/`App.delete`'s own `@@allow`
// (`owner`/`admin` only, `App.delete` `owner`-only) admits no
// `auth().kind == "app"` clause, so this screen's writes only became
// reachable once #211 forwarded the signed-in human's own session token.
//
// # "Show secret once" — what this screen actually does about it
//
// `provisionAppClient` returns `privateKeyPem` in exactly one response.
// [`ProvisionKeyDialog`] renders it from the mutation's own `data` (a
// `useMutation` hook's result lives in that hook's component-local React
// state, not TanStack Query's shared cache — nothing else in this app can
// read it back), never writes it into `localStorage`/`sessionStorage`,
// never routes it through a toast (`toast()` calls here are for outcomes
// only — "client provisioned", never the key itself), and calling
// `provisionMutation.reset()` on close clears the hook's own held `data`
// immediately rather than leaving it retrievable by, say, a browser
// back/forward cache restoring the component. The dialog requires an
// explicit "I've saved this key — close" click (not a bare "Close") so
// dismissing it isn't a single careless click.
//
// # Retire — the coarse fallback, stated on screen, not just in a comment
//
// There is no per-client key-history model, so retiring is immediate and
// total: the moment it succeeds, the old key stops authenticating. See
// `@vsms/gateway/app-clients.ts`'s own module doc for the full reasoning
// and what a caller wanting zero-downtime rotation has to do instead
// (provision the replacement first, migrate the integration, retire the
// old one last) — this screen's own copy says the same thing next to the
// button, not just in source.

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
import { useEffect, useState } from "react";
import { ConsoleNav } from "../console-nav";

type RouterOutputs = inferRouterOutputs<AppRouter>;
type AppListItem = RouterOutputs["apps"]["list"][number];
type AppClientListItem = RouterOutputs["appClients"]["listForApp"][number];

interface AppFormState {
  name: string;
  description: string;
  monthlyQuota: string;
  ipAllowlist: string;
  transliterateToGsm7: boolean;
  active: boolean;
}

function toIpAllowlistLines(entries: string[]): string {
  return entries.join("\n");
}

function parseIpAllowlistLines(text: string): string[] {
  return text
    .split(/\r?\n|,/)
    .map((line) => line.trim())
    .filter((line) => line.length > 0);
}

function ProvisionClientDialog({
  appId,
  open,
  onOpenChange,
}: {
  appId: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const utils = trpc.useUtils();
  const [label, setLabel] = useState("");
  const [scopesText, setScopesText] = useState("sms:send sms:read");
  const provisionMutation = trpc.appClients.provision.useMutation({
    onSuccess: () => {
      void utils.appClients.listForApp.invalidate({ appId });
    },
  });

  function close() {
    onOpenChange(false);
  }

  function closeAndClear() {
    // Clears the mutation hook's own held `data` (the private key) —
    // see this screen's own module doc.
    provisionMutation.reset();
    setLabel("");
    onOpenChange(false);
  }

  const key = provisionMutation.data;

  return (
    <Dialog open={open} onOpenChange={(next) => (next ? undefined : closeAndClear())}>
      <DialogContent className="max-w-[560px]">
        <DialogHeader>
          <DialogTitle>Provision a service-account client</DialogTitle>
          <DialogDescription>
            The private key is shown exactly once. It is never stored anywhere by this console or by
            sms-api — copy it now, or the client has to be retired and re-provisioned.
          </DialogDescription>
        </DialogHeader>

        {key === undefined && (
          <div className="flex flex-col gap-4">
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="client-label">Label</Label>
              <Input
                id="client-label"
                placeholder="e.g. billing-service"
                value={label}
                onChange={(e) => setLabel(e.target.value)}
              />
            </div>
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="client-scopes">Scopes (space-separated)</Label>
              <Input
                id="client-scopes"
                value={scopesText}
                onChange={(e) => setScopesText(e.target.value)}
              />
              <p className="text-caption text-subtle-foreground">
                e.g. <span className="font-mono">sms:send sms:read</span>
              </p>
            </div>
            {provisionMutation.isError && (
              <div className="rounded-sm border border-state-danger-border bg-state-danger-bg px-3 py-2 text-caption text-state-danger-fg">
                {provisionMutation.error.message}
              </div>
            )}
          </div>
        )}

        {key !== undefined && (
          <div className="flex flex-col gap-3">
            <div className="rounded-sm border border-edge bg-surface-2 px-3 py-2 text-caption text-muted-foreground">
              Client id: <span className="font-mono text-foreground">{key.clientId}</span>
            </div>
            <div className="flex flex-col gap-1.5">
              <Label>Private key (PKCS#8 PEM) — save this now</Label>
              <Textarea
                readOnly
                rows={12}
                className="font-mono text-caption"
                value={key.privateKeyPem}
              />
              <Button
                type="button"
                variant="secondary"
                size="sm"
                onClick={() => {
                  void navigator.clipboard.writeText(key.privateKeyPem);
                  toast({ title: "Private key copied", variant: "success" });
                }}
              >
                Copy key
              </Button>
            </div>
          </div>
        )}

        <DialogFooter>
          {key === undefined ? (
            <>
              <Button type="button" variant="ghost" onClick={close}>
                Cancel
              </Button>
              <Button
                type="button"
                disabled={label.trim().length === 0 || provisionMutation.isPending}
                onClick={() =>
                  provisionMutation.mutate({
                    appId,
                    label: label.trim(),
                    scopes: scopesText.split(/\s+/).filter((s) => s.length > 0),
                  })
                }
              >
                {provisionMutation.isPending ? "Provisioning…" : "Provision"}
              </Button>
            </>
          ) : (
            <Button type="button" onClick={closeAndClear}>
              I&apos;ve saved this key — close
            </Button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function AppClientsPanel({ appId }: { appId: string }) {
  const listQuery = trpc.appClients.listForApp.useQuery({ appId });
  const utils = trpc.useUtils();
  const [provisionOpen, setProvisionOpen] = useState(false);
  const [retiringId, setRetiringId] = useState<string | null>(null);

  const retireMutation = trpc.appClients.retire.useMutation({
    onSuccess: () => {
      toast({ title: "Client retired", variant: "success" });
      setRetiringId(null);
      void utils.appClients.listForApp.invalidate({ appId });
    },
  });

  return (
    <div className="flex flex-col gap-3">
      <div className="flex items-center justify-between">
        <h3 className="font-medium text-body text-foreground">Service-account clients</h3>
        <Button type="button" size="sm" onClick={() => setProvisionOpen(true)}>
          Provision client
        </Button>
      </div>

      {listQuery.isError && (
        <div className="rounded-sm border border-state-danger-border bg-state-danger-bg px-3 py-2 text-caption text-state-danger-fg">
          {listQuery.error.message}
        </div>
      )}

      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>Label</TableHead>
            <TableHead>Client id</TableHead>
            <TableHead>Scopes</TableHead>
            <TableHead>Active</TableHead>
            <TableHead align="end">Actions</TableHead>
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
                <InlineEmptyState message="No clients provisioned for this app yet." />
              </td>
            </tr>
          )}
          {listQuery.data?.map((client: AppClientListItem) => (
            <TableRow key={client.id}>
              <TableCell>{client.label}</TableCell>
              <TableCell mono>{client.clientId}</TableCell>
              <TableCell mono className="text-caption">
                {client.scopes.trim()}
              </TableCell>
              <TableCell>
                {client.active ? (
                  <span className="text-state-success-fg">active</span>
                ) : (
                  <span className="text-muted-foreground">retired</span>
                )}
              </TableCell>
              <TableCell align="end">
                {client.active && (
                  <Button
                    type="button"
                    variant="ghost"
                    size="sm"
                    onClick={() => setRetiringId(client.id)}
                  >
                    Retire
                  </Button>
                )}
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>

      <ProvisionClientDialog appId={appId} open={provisionOpen} onOpenChange={setProvisionOpen} />

      <Dialog open={retiringId !== null} onOpenChange={(open) => !open && setRetiringId(null)}>
        <DialogContent className="max-w-[480px]">
          <DialogHeader>
            <DialogTitle>Retire this client?</DialogTitle>
            <DialogDescription>
              This is immediate and total — there is no overlap window. The client&apos;s current
              key stops authenticating the instant this succeeds. If a live integration still uses
              it, provision its replacement and migrate first.
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button type="button" variant="ghost" onClick={() => setRetiringId(null)}>
              Cancel
            </Button>
            <Button
              type="button"
              variant="destructive"
              disabled={retireMutation.isPending}
              onClick={() => {
                const client = listQuery.data?.find((c) => c.id === retiringId);
                if (client === undefined) return;
                retireMutation.mutate({ id: client.id, etag: String(client.version) });
              }}
            >
              {retireMutation.isPending ? "Retiring…" : "Retire client"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}

function CreateAppDialog({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const utils = trpc.useUtils();
  const [name, setName] = useState("");
  const [slug, setSlug] = useState("");
  const [monthlyQuota, setMonthlyQuota] = useState("10000");
  const createMutation = trpc.apps.create.useMutation({
    onSuccess: () => {
      toast({ title: "App created", variant: "success" });
      setName("");
      setSlug("");
      onOpenChange(false);
      void utils.apps.list.invalidate();
    },
  });

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-[480px]">
        <DialogHeader>
          <DialogTitle>New app</DialogTitle>
        </DialogHeader>
        <div className="flex flex-col gap-4">
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="app-name">Name</Label>
            <Input id="app-name" value={name} onChange={(e) => setName(e.target.value)} />
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="app-slug">Slug</Label>
            <Input
              id="app-slug"
              placeholder="lowercase-with-hyphens"
              value={slug}
              onChange={(e) => setSlug(e.target.value)}
            />
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="app-quota">Monthly quota</Label>
            <Input
              id="app-quota"
              type="number"
              min="0"
              value={monthlyQuota}
              onChange={(e) => setMonthlyQuota(e.target.value)}
            />
          </div>
          {createMutation.isError && (
            <div className="rounded-sm border border-state-danger-border bg-state-danger-bg px-3 py-2 text-caption text-state-danger-fg">
              {createMutation.error.message}
            </div>
          )}
        </div>
        <DialogFooter>
          <Button type="button" variant="ghost" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button
            type="button"
            disabled={
              name.trim().length === 0 || slug.trim().length === 0 || createMutation.isPending
            }
            onClick={() =>
              createMutation.mutate({
                name: name.trim(),
                slug: slug.trim(),
                monthlyQuota: Number(monthlyQuota),
                ipAllowlist: [],
                transliterateToGsm7: false,
              })
            }
          >
            {createMutation.isPending ? "Creating…" : "Create"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

export function AppsScreen() {
  const listQuery = trpc.apps.list.useQuery();
  const utils = trpc.useUtils();
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [createOpen, setCreateOpen] = useState(false);
  const [deleteConfirmId, setDeleteConfirmId] = useState<string | null>(null);

  const detailQuery = trpc.apps.get.useQuery(
    { id: selectedId ?? "" },
    { enabled: selectedId !== null },
  );

  const [form, setForm] = useState<AppFormState | null>(null);

  useEffect(() => {
    if (detailQuery.data?.data !== undefined) {
      const d = detailQuery.data.data;
      setForm({
        name: d.name,
        description: d.description ?? "",
        monthlyQuota: String(d.monthlyQuota),
        ipAllowlist: toIpAllowlistLines(
          d.ipAllowlist
            .trim()
            .split(/\s+/)
            .filter((e) => e.length > 0),
        ),
        transliterateToGsm7: d.transliterateToGsm7,
        active: d.active,
      });
    }
  }, [detailQuery.data]);

  const updateMutation = trpc.apps.update.useMutation({
    onSuccess: () => {
      toast({ title: "App saved", variant: "success" });
      void utils.apps.list.invalidate();
      void utils.apps.get.invalidate({ id: selectedId ?? "" });
    },
  });

  const deleteMutation = trpc.apps.delete.useMutation({
    onSuccess: () => {
      toast({ title: "App deleted", variant: "success" });
      setSelectedId(null);
      setDeleteConfirmId(null);
      void utils.apps.list.invalidate();
    },
  });

  function closeDetail() {
    setSelectedId(null);
    setForm(null);
    updateMutation.reset();
  }

  function save() {
    if (selectedId === null || form === null || detailQuery.data?.etag === undefined) return;
    updateMutation.mutate({
      id: selectedId,
      etag: detailQuery.data.etag,
      name: form.name,
      description: form.description.length > 0 ? form.description : undefined,
      monthlyQuota: Number(form.monthlyQuota),
      ipAllowlist: parseIpAllowlistLines(form.ipAllowlist),
      transliterateToGsm7: form.transliterateToGsm7,
      active: form.active,
    });
  }

  return (
    <main className="mx-auto flex max-w-[1200px] flex-col gap-6 px-6 py-10">
      <header className="flex items-start justify-between gap-4 border-edge border-b pb-6">
        <div>
          <p className="font-mono text-micro text-subtle-foreground tracking-[0.03em]">
            vsms admin console
          </p>
          <h1 className="mt-1 font-medium text-foreground text-title">Apps</h1>
          <p className="mt-1 max-w-xl text-body text-muted-foreground">
            Every integrated product, its quota, and its service-account clients.
          </p>
        </div>
        <ConsoleNav current="/apps" />
      </header>

      <div className="flex items-center justify-between">
        <div className="rounded-sm border border-edge bg-surface-2 px-3 py-2 text-caption text-muted-foreground">
          Reads and writes act as you — saving requires your own role to carry{" "}
          <span className="font-mono text-foreground">app:write</span> (owner and admin by default),
          and provisioning/retiring a client needs{" "}
          <span className="font-mono text-foreground">user:manage</span>-adjacent trust: this
          console&apos;s own permission table gates it at{" "}
          <span className="font-mono text-foreground">owner</span>/
          <span className="font-mono text-foreground">admin</span> only.
        </div>
        <Button type="button" onClick={() => setCreateOpen(true)}>
          New app
        </Button>
      </div>

      {listQuery.isError && (
        <div className="rounded-sm border border-state-danger-border bg-state-danger-bg px-3 py-2 text-caption text-state-danger-fg">
          Could not read apps: {listQuery.error.message}
        </div>
      )}

      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>Active</TableHead>
            <TableHead>Name</TableHead>
            <TableHead>Slug</TableHead>
            <TableHead align="end">Monthly quota</TableHead>
            <TableHead>Transliterate to GSM-7</TableHead>
            <TableHead align="end">Updated</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {listQuery.isLoading &&
            Array.from({ length: 4 }).map((_, i) => (
              // biome-ignore lint/suspicious/noArrayIndexKey: static skeleton rows, never reordered or diffed
              <TableRow key={i}>
                <TableCell colSpan={6}>
                  <Skeleton className="h-4 w-full" />
                </TableCell>
              </TableRow>
            ))}

          {!listQuery.isLoading && (listQuery.data?.length ?? 0) === 0 && (
            <tr>
              <td colSpan={6}>
                <InlineEmptyState message="No apps yet." />
              </td>
            </tr>
          )}

          {listQuery.data?.map((app: AppListItem) => (
            <TableRow key={app.id} className="cursor-pointer" onClick={() => setSelectedId(app.id)}>
              <TableCell>
                {app.active ? (
                  <span className="text-state-success-fg">yes</span>
                ) : (
                  <span className="text-muted-foreground">no</span>
                )}
              </TableCell>
              <TableCell>{app.name}</TableCell>
              <TableCell mono>{app.slug}</TableCell>
              <TableCell align="end" mono>
                {app.monthlyQuota.toLocaleString()}
              </TableCell>
              <TableCell>{app.transliterateToGsm7 ? "on" : "off"}</TableCell>
              <TableCell align="end">
                <TimestampDisplay value={app.updatedAt} />
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>

      <CreateAppDialog open={createOpen} onOpenChange={setCreateOpen} />

      <Dialog open={selectedId !== null} onOpenChange={(open) => !open && closeDetail()}>
        <DialogContent className="max-w-[720px]">
          <DialogHeader>
            <DialogTitle>
              {detailQuery.data?.data !== undefined ? detailQuery.data.data.name : "App"}
            </DialogTitle>
            <DialogDescription>
              {selectedId !== null && <IdDisplay value={selectedId} variant="full" />}
            </DialogDescription>
          </DialogHeader>

          {detailQuery.isLoading && <Skeleton className="h-32 w-full" />}

          {detailQuery.data?.data !== undefined && form !== null && (
            <div className="flex max-h-[70vh] flex-col gap-6 overflow-y-auto pr-1">
              <div className="flex flex-col gap-4">
                <div className="grid grid-cols-2 gap-3">
                  <div className="flex flex-col gap-1.5">
                    <Label htmlFor="app-edit-name">Name</Label>
                    <Input
                      id="app-edit-name"
                      value={form.name}
                      onChange={(e) => setForm({ ...form, name: e.target.value })}
                    />
                  </div>
                  <div className="flex flex-col gap-1.5">
                    <Label>Slug</Label>
                    <Input value={detailQuery.data.data.slug} disabled />
                  </div>
                </div>

                <div className="flex flex-col gap-1.5">
                  <Label htmlFor="app-edit-description">Description</Label>
                  <Textarea
                    id="app-edit-description"
                    rows={2}
                    value={form.description}
                    onChange={(e) => setForm({ ...form, description: e.target.value })}
                  />
                </div>

                <div className="grid grid-cols-2 gap-3">
                  <div className="flex flex-col gap-1.5">
                    <Label htmlFor="app-edit-quota">Monthly quota</Label>
                    <Input
                      id="app-edit-quota"
                      type="number"
                      min="0"
                      value={form.monthlyQuota}
                      onChange={(e) => setForm({ ...form, monthlyQuota: e.target.value })}
                    />
                  </div>
                  <div className="flex items-end gap-4 pb-2">
                    <label className="flex items-center gap-2 text-caption text-foreground">
                      <input
                        type="checkbox"
                        checked={form.transliterateToGsm7}
                        onChange={(e) =>
                          setForm({ ...form, transliterateToGsm7: e.target.checked })
                        }
                      />
                      Transliterate to GSM-7
                    </label>
                    <label className="flex items-center gap-2 text-caption text-foreground">
                      <input
                        type="checkbox"
                        checked={form.active}
                        onChange={(e) => setForm({ ...form, active: e.target.checked })}
                      />
                      Active
                    </label>
                  </div>
                </div>

                <div className="flex flex-col gap-1.5">
                  <Label htmlFor="app-edit-allowlist">
                    IP allowlist (one CIDR per line — blank = unrestricted)
                  </Label>
                  <Textarea
                    id="app-edit-allowlist"
                    rows={3}
                    className="font-mono text-caption"
                    value={form.ipAllowlist}
                    onChange={(e) => setForm({ ...form, ipAllowlist: e.target.value })}
                  />
                </div>

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
                    Delete app
                  </Button>
                  <Button type="button" disabled={updateMutation.isPending} onClick={save}>
                    {updateMutation.isPending ? "Saving…" : "Save"}
                  </Button>
                </div>
              </div>

              <div className="border-edge border-t pt-4">
                <AppClientsPanel appId={selectedId ?? ""} />
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
            <DialogTitle>Delete this app?</DialogTitle>
            <DialogDescription>
              This soft-deletes the row (owner only) — existing messages and clients referencing it
              are untouched, but the app stops being usable for new sends.
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
    </main>
  );
}
