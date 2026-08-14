"use client";

// The Opt-outs screen (#58): search by hashed MSISDN, record a new
// opt-out, and browse recent activity.
//
// Console redesign (docs/design/console-redesign.md, Phase 2
// "Operations"): dropped the old `ConsoleNav` hand-rolled link strip for
// `ConsoleShell`'s own sidebar/top bar, and added a row-click
// `QuickDetailDrawer` (§3/D14) for the recent-activity table — the full
// record (`msisdnHash`, `scope`, `reason`, both timestamps) is one tap
// away rather than needing every column visible on a phone; `Remove`
// lives in both the table row (fast path for an operator already scanning
// the list) and the drawer footer (reachable from the peek too), both
// driving the same confirm `Dialog`.
//
// Record-opt-out stays a `Dialog`, not a drawer: four fields, no
// sub-navigation, no nested history — exactly §3's own "short
// single-purpose forms with no sub-navigation (rename, confirm-requeue)"
// bucket, not "the full record plus an edit form" a `MoreDetailDrawer` is
// for.
//
// # Search is a procedure, not a filtered list, for a structural reason
//
// `OptOut.msisdnHash` is a peppered HMAC (`SMS_HASH_PEPPER`) this console
// never has access to — there is no client-side way to compute the hash a
// `GET /opt_outs?msisdnHash=...` filter would need, so the search box
// calls `optOuts.search` (`POST /$procs/searchOptOutByMsisdn`), which
// hashes server-side. See `@vsms/gateway/opt-outs.ts`'s own module doc.
//
// # What "not found" honestly means here — said on screen, not just in a
// # comment
//
// A pepper rotation orphans every `msisdnHash` computed under the old
// pepper, silently and permanently (`OPEN_QUESTIONS.md` §3.1). This screen
// cannot tell "never opted out" apart from "opted out before the last
// rotation" — the banner below the search result says so on every search,
// not only when a result comes back empty (an operator reading a *found*
// result also needs to know the flip side could exist unseen for a
// *different* number).

import type { inferRouterOutputs } from "@trpc/server";
import type { AppRouter } from "@vsms/api";
import { trpc } from "@vsms/hooks";
import {
  Button,
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  IdDisplay,
  InlineEmptyState,
  Input,
  Label,
  MsisdnDisplay,
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
import { ChevronRight } from "lucide-react";
import { type ReactNode, useState } from "react";

type RouterOutputs = inferRouterOutputs<AppRouter>;
type OptOutListItem = RouterOutputs["optOuts"]["list"][number];

const SOURCES = ["inbound_stop", "admin", "import", "operator"] as const;

function SearchPanel() {
  const utils = trpc.useUtils();
  const [msisdn, setMsisdn] = useState("");
  const [searchedFor, setSearchedFor] = useState<string | null>(null);
  const searchQuery = trpc.optOuts.search.useQuery(
    { msisdn: searchedFor ?? "" },
    { enabled: searchedFor !== null },
  );

  return (
    <div className="flex flex-col gap-3 rounded-sm border border-edge bg-surface-2 p-4">
      <div className="flex flex-col items-stretch gap-2 sm:flex-row sm:items-end">
        <div className="flex flex-1 flex-col gap-1.5">
          <Label htmlFor="opt-out-search">Search by MSISDN</Label>
          <Input
            id="opt-out-search"
            placeholder="+237677123456"
            value={msisdn}
            onChange={(e) => setMsisdn(e.target.value)}
          />
        </div>
        <Button
          type="button"
          disabled={msisdn.trim().length === 0}
          onClick={() => {
            setSearchedFor(msisdn.trim());
            void utils.optOuts.search.invalidate();
          }}
        >
          Search
        </Button>
      </div>

      {searchedFor !== null && (
        <div className="rounded-sm border border-edge bg-surface-1 p-3">
          {searchQuery.isLoading && <Skeleton className="h-6 w-full" />}
          {searchQuery.isError && (
            <p className="text-caption text-state-danger-fg">{searchQuery.error.message}</p>
          )}
          {searchQuery.data !== undefined && searchQuery.data.optOut !== undefined && (
            <div className="flex flex-col gap-1 text-caption">
              <p className="text-state-danger-fg">
                Opted out — source{" "}
                <span className="font-mono">{searchQuery.data.optOut.source}</span>, scope{" "}
                <span className="font-mono">{searchQuery.data.optOut.scope}</span>
              </p>
              <p className="text-muted-foreground">
                <TimestampDisplay value={searchQuery.data.optOut.optedOutAt} />
                {searchQuery.data.optOut.reason !== undefined && (
                  <> — {searchQuery.data.optOut.reason}</>
                )}
              </p>
            </div>
          )}
          {searchQuery.data !== undefined && searchQuery.data.optOut === undefined && (
            <p className="text-caption text-state-success-fg">No opt-out found for that number.</p>
          )}
          <p className="mt-2 text-micro text-subtle-foreground">
            This can never distinguish &quot;never opted out&quot; from &quot;opted out before the
            hash pepper was last rotated&quot; — a rotation orphans older hashes silently and
            permanently. Treat a &quot;not found&quot; result as inconclusive for a number with any
            history predating a known rotation, not as proof.
          </p>
        </div>
      )}
    </div>
  );
}

function RecordDialog({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const utils = trpc.useUtils();
  const [msisdn, setMsisdn] = useState("");
  const [source, setSource] = useState<(typeof SOURCES)[number]>("admin");
  const [scope, setScope] = useState("all");
  const [reason, setReason] = useState("");

  const recordMutation = trpc.optOuts.record.useMutation({
    onSuccess: () => {
      toast({ title: "Opt-out recorded", variant: "success" });
      setMsisdn("");
      setReason("");
      onOpenChange(false);
      void utils.optOuts.list.invalidate();
    },
  });

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-[480px]">
        <DialogHeader>
          <DialogTitle>Record an opt-out</DialogTitle>
        </DialogHeader>
        <div className="flex flex-col gap-4">
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="record-msisdn">MSISDN</Label>
            <Input
              id="record-msisdn"
              placeholder="+237677123456"
              value={msisdn}
              onChange={(e) => setMsisdn(e.target.value)}
            />
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="record-source">Source</Label>
            <Select value={source} onValueChange={(v) => setSource(v as (typeof SOURCES)[number])}>
              <SelectTrigger id="record-source">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {SOURCES.map((s) => (
                  <SelectItem key={s} value={s}>
                    {s}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="record-scope">Scope</Label>
            <Input id="record-scope" value={scope} onChange={(e) => setScope(e.target.value)} />
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="record-reason">Reason (optional)</Label>
            <Input id="record-reason" value={reason} onChange={(e) => setReason(e.target.value)} />
          </div>
          {recordMutation.isError && (
            <div className="rounded-sm border border-state-danger-border bg-state-danger-bg px-3 py-2 text-caption text-state-danger-fg">
              {recordMutation.error.message}
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
              msisdn.trim().length === 0 || scope.trim().length === 0 || recordMutation.isPending
            }
            onClick={() =>
              recordMutation.mutate({
                msisdn: msisdn.trim(),
                source,
                scope: scope.trim(),
                reason: reason.trim().length > 0 ? reason.trim() : undefined,
              })
            }
          >
            {recordMutation.isPending ? "Recording…" : "Record"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function OptOutDetailField({ label, value }: { label: string; value: ReactNode }) {
  return (
    <div className="flex flex-col gap-0.5 border-edge-subtle border-b py-2 last:border-b-0">
      <dt className="text-caption text-subtle-foreground">{label}</dt>
      <dd className="text-body text-foreground">{value}</dd>
    </div>
  );
}

export function OptOutsScreen() {
  const listQuery = trpc.optOuts.list.useQuery({});
  const utils = trpc.useUtils();
  const [recordOpen, setRecordOpen] = useState(false);
  const [deleteConfirmId, setDeleteConfirmId] = useState<string | null>(null);
  const [detailRow, setDetailRow] = useState<OptOutListItem | null>(null);

  const deleteMutation = trpc.optOuts.delete.useMutation({
    onSuccess: () => {
      toast({ title: "Opt-out removed", variant: "success" });
      setDeleteConfirmId(null);
      setDetailRow(null);
      void utils.optOuts.list.invalidate();
    },
  });

  const liveDetailRow =
    detailRow === null
      ? null
      : (listQuery.data?.find((row) => row.id === detailRow.id) ?? detailRow);

  return (
    <div className="flex flex-col gap-6">
      <header className="flex flex-col gap-1">
        <h1 className="font-medium text-foreground text-title">Opt-outs</h1>
        <p className="max-w-xl text-body text-muted-foreground">
          Search by MSISDN, record a new opt-out, or browse recent activity.
        </p>
      </header>

      <SearchPanel />

      <div className="flex items-center justify-between">
        <h2 className="font-medium text-body text-foreground">Recent</h2>
        <Button type="button" onClick={() => setRecordOpen(true)}>
          Record opt-out
        </Button>
      </div>

      {listQuery.isError && (
        <div className="rounded-sm border border-state-danger-border bg-state-danger-bg px-3 py-2 text-caption text-state-danger-fg">
          Could not read opt-outs: {listQuery.error.message}
        </div>
      )}

      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>MSISDN</TableHead>
            <TableHead>Source</TableHead>
            <TableHead className="hidden md:table-cell">Scope</TableHead>
            <TableHead className="hidden lg:table-cell">Reason</TableHead>
            <TableHead align="end" className="hidden sm:table-cell">
              Opted out
            </TableHead>
            <TableHead align="end">Actions</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {listQuery.isLoading && (
            <TableRow>
              <TableCell colSpan={6}>
                <Skeleton className="h-4 w-full" />
              </TableCell>
            </TableRow>
          )}
          {!listQuery.isLoading && (listQuery.data?.length ?? 0) === 0 && (
            <tr>
              <td colSpan={6}>
                <InlineEmptyState message="No opt-outs recorded yet." />
              </td>
            </tr>
          )}
          {listQuery.data?.map((row: OptOutListItem) => (
            <TableRow
              key={row.id}
              tabIndex={0}
              role="button"
              aria-label={`View details for opt-out ${row.msisdn}`}
              className="cursor-pointer"
              onClick={() => setDetailRow(row)}
              onKeyDown={(e) => {
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  setDetailRow(row);
                }
              }}
            >
              <TableCell>
                <MsisdnDisplay value={row.msisdn} />
              </TableCell>
              <TableCell mono>{row.source}</TableCell>
              <TableCell mono className="hidden md:table-cell">
                {row.scope}
              </TableCell>
              <TableCell className="hidden max-w-[240px] truncate text-caption text-muted-foreground lg:table-cell">
                {row.reason ?? "—"}
              </TableCell>
              <TableCell align="end" className="hidden sm:table-cell">
                <TimestampDisplay value={row.optedOutAt} />
              </TableCell>
              <TableCell align="end">
                <div className="flex items-center justify-end gap-1.5">
                  <Button
                    type="button"
                    variant="ghost"
                    size="sm"
                    onClick={(e) => {
                      e.stopPropagation();
                      setDeleteConfirmId(row.id);
                    }}
                  >
                    Remove
                  </Button>
                  <ChevronRight
                    size={14}
                    strokeWidth={1.5}
                    aria-hidden="true"
                    className="text-subtle-foreground"
                  />
                </div>
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>

      <RecordDialog open={recordOpen} onOpenChange={setRecordOpen} />

      <QuickDetailDrawer
        open={liveDetailRow !== null}
        onOpenChange={(open) => !open && setDetailRow(null)}
        title={liveDetailRow != null ? liveDetailRow.msisdn : "Opt-out"}
        description={liveDetailRow != null ? `Opt-out ${liveDetailRow.id}` : undefined}
        footer={
          liveDetailRow != null ? (
            <Button
              type="button"
              variant="destructive"
              size="sm"
              onClick={() => setDeleteConfirmId(liveDetailRow.id)}
            >
              Remove
            </Button>
          ) : undefined
        }
      >
        {liveDetailRow != null && (
          <dl className="flex flex-col">
            <OptOutDetailField
              label="MSISDN"
              value={<MsisdnDisplay value={liveDetailRow.msisdn} />}
            />
            <OptOutDetailField
              label="MSISDN hash"
              value={<IdDisplay value={liveDetailRow.msisdnHash} variant="full" />}
            />
            <OptOutDetailField
              label="Source"
              value={<span className="font-mono">{liveDetailRow.source}</span>}
            />
            <OptOutDetailField
              label="Scope"
              value={<span className="font-mono">{liveDetailRow.scope}</span>}
            />
            <OptOutDetailField
              label="Reason"
              value={
                liveDetailRow.reason != null ? (
                  <span className="whitespace-pre-wrap break-words">{liveDetailRow.reason}</span>
                ) : (
                  "—"
                )
              }
            />
            <OptOutDetailField
              label="Opted out at"
              value={<TimestampDisplay value={liveDetailRow.optedOutAt} />}
            />
            <OptOutDetailField
              label="Recorded at"
              value={<TimestampDisplay value={liveDetailRow.createdAt} />}
            />
            <OptOutDetailField
              label="Id"
              value={<IdDisplay value={liveDetailRow.id} variant="full" />}
            />
          </dl>
        )}
      </QuickDetailDrawer>

      <Dialog
        open={deleteConfirmId !== null}
        onOpenChange={(open) => !open && setDeleteConfirmId(null)}
      >
        <DialogContent className="max-w-[440px]">
          <DialogHeader>
            <DialogTitle>Remove this opt-out?</DialogTitle>
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
              {deleteMutation.isPending ? "Removing…" : "Remove"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
