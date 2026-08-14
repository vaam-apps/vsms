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
// driving the same confirm dialog.
//
// Record-opt-out stays a dialog, not a drawer: four fields, no
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
//
// R6 (AGENTS.md): this file is the smart component. Markup and classes
// moved verbatim into `components/opt-outs-search-panel.tsx`,
// `components/opt-outs-table.tsx`, `components/opt-out-detail-fields.tsx`,
// `components/opt-outs-toolbar.tsx`, `components/record-opt-out-
// dialog.tsx` and `components/remove-confirm-dialog.tsx`.
//
// Two real fixes, not just a move:
//
// 1. **`SearchPanel` (now `OptOutsSearchPanel`) called `trpc.optOuts.
//    search.useQuery` itself.** A dumb component fetching its own data is
//    an R6 violation independent of where its classes live. The query now
//    lives here; the dumb component only renders whatever `result` it's
//    handed. The same was true of the old `RecordDialog`'s own `trpc.
//    optOuts.record.useMutation` call — fixed the same way, moved to this
//    file, mutation-success handling (toast, form reset, closing the
//    dialog, invalidating the list) included.
// 2. **`recordOpen`/`deleteConfirmId`/`detailRow` were three separate
//    `useState` calls** — R6's own worked "wrong example" is this exact
//    trio, verbatim. `detailRow` was the worse of the two "holds a
//    server-row copy" cases in this route group (`jobs-screen.tsx`'s
//    `detailJob`/`confirmTarget` being the other): it needed its own `??
//    detailRow` staleness fallback. All three now live in the URL
//    (`nuqs`, `history: "replace"` — opening a dialog while browsing
//    shouldn't grow the back-button trail), and `detail` holds only the
//    row's **id** — `detailRow` is derived fresh from `listQuery.data` on
//    every render, so there is no copy left to go stale and the fallback
//    is gone.
//
// One thing that deliberately did NOT move to the URL: the search box's
// own `msisdn`/`searchedFor` text. An MSISDN is exactly the kind of value
// this screen's own banner warns is sensitive (`msisdnHash` is a peppered
// HMAC precisely because a raw MSISDN is personal data) — putting it in a
// URL query string means browser history, server access logs and a
// pasted/shared link would all carry it in the clear, which R6's own
// "when it is genuinely unclear, leave it in code with a comment saying
// why" calls the safer default. Kept as plain `useState`, which R6's
// "avoid useState" section explicitly allows for "genuinely ephemeral,
// single-value presentational state."

import { zodResolver } from "@hookform/resolvers/zod";
import { trpc } from "@vsms/hooks";
import {
  Button,
  InlineBanner,
  QuickDetailDrawer,
  ScreenHeader,
  ScreenStack,
  toast,
} from "@vsms/ui";
import { parseAsBoolean, parseAsString, useQueryStates } from "nuqs";
import { useState } from "react";
import { useForm } from "react-hook-form";
import { OptOutDetailFields } from "./components/opt-out-detail-fields";
import { OptOutsSearchPanel } from "./components/opt-outs-search-panel";
import { type OptOutListItem, OptOutsTable } from "./components/opt-outs-table";
import { OptOutsToolbar } from "./components/opt-outs-toolbar";
import { RecordOptOutDialog } from "./components/record-opt-out-dialog";
import { RemoveConfirmDialog } from "./components/remove-confirm-dialog";
import {
  RECORD_OPT_OUT_DEFAULTS,
  type RecordOptOutFormValues,
  recordOptOutSchema,
} from "./record-opt-out-schema";

export function OptOutsScreen() {
  const listQuery = trpc.optOuts.list.useQuery({});
  const utils = trpc.useUtils();

  const [urlState, setUrlState] = useQueryStates(
    {
      record: parseAsBoolean.withDefault(false),
      remove: parseAsString,
      detail: parseAsString,
    },
    { history: "replace" },
  );

  // Ephemeral, single-value, and deliberately not URL state — see this
  // file's own module doc.
  const [msisdn, setMsisdn] = useState("");
  const [searchedFor, setSearchedFor] = useState<string | null>(null);
  const searchQuery = trpc.optOuts.search.useQuery(
    { msisdn: searchedFor ?? "" },
    { enabled: searchedFor !== null },
  );

  const form = useForm<RecordOptOutFormValues>({
    resolver: zodResolver(recordOptOutSchema),
    defaultValues: RECORD_OPT_OUT_DEFAULTS,
  });

  const recordMutation = trpc.optOuts.record.useMutation({
    onSuccess: () => {
      toast({ title: "Opt-out recorded", variant: "success" });
      form.reset({ ...form.getValues(), msisdn: "", reason: "" });
      void setUrlState({ record: false });
      void utils.optOuts.list.invalidate();
    },
  });

  const deleteMutation = trpc.optOuts.delete.useMutation({
    onSuccess: () => {
      toast({ title: "Opt-out removed", variant: "success" });
      void setUrlState({ remove: null, detail: null });
      void utils.optOuts.list.invalidate();
    },
  });

  const items: OptOutListItem[] = listQuery.data ?? [];
  const detailRow =
    urlState.detail === null ? null : (items.find((row) => row.id === urlState.detail) ?? null);

  function openRecord() {
    void setUrlState({ record: true });
  }

  function closeRecord(open: boolean) {
    if (!open) void setUrlState({ record: false });
  }

  function openRemoveConfirm(id: string) {
    void setUrlState({ remove: id });
  }

  function closeRemoveConfirm() {
    void setUrlState({ remove: null });
  }

  function openDetail(row: OptOutListItem) {
    void setUrlState({ detail: row.id });
  }

  function closeDetail() {
    void setUrlState({ detail: null });
  }

  function runSearch() {
    setSearchedFor(msisdn.trim());
    void utils.optOuts.search.invalidate();
  }

  function submitRecord(values: RecordOptOutFormValues) {
    recordMutation.mutate({
      msisdn: values.msisdn,
      source: values.source,
      scope: values.scope,
      reason: values.reason.length > 0 ? values.reason : undefined,
    });
  }

  function confirmRemove() {
    if (urlState.remove === null) return;
    deleteMutation.mutate({ id: urlState.remove });
  }

  return (
    <ScreenStack>
      <ScreenHeader
        title="Opt-outs"
        description="Search by MSISDN, record a new opt-out, or browse recent activity."
      />

      <OptOutsSearchPanel
        msisdn={msisdn}
        onMsisdnChange={setMsisdn}
        canSearch={msisdn.trim().length > 0}
        onSearch={runSearch}
        searchedFor={searchedFor}
        isLoading={searchQuery.isLoading}
        isError={searchQuery.isError}
        errorMessage={searchQuery.error?.message}
        result={searchQuery.data}
      />

      <OptOutsToolbar onRecordClick={openRecord} />

      {listQuery.isError && (
        <InlineBanner variant="danger">
          Could not read opt-outs: {listQuery.error.message}
        </InlineBanner>
      )}

      <OptOutsTable
        items={items}
        isLoading={listQuery.isLoading}
        onRowClick={openDetail}
        onRemoveClick={(row) => openRemoveConfirm(row.id)}
      />

      <RecordOptOutDialog
        open={urlState.record}
        onOpenChange={closeRecord}
        form={form}
        onSubmit={submitRecord}
        isPending={recordMutation.isPending}
        errorMessage={recordMutation.error?.message}
      />

      <QuickDetailDrawer
        open={detailRow !== null}
        onOpenChange={(open) => !open && closeDetail()}
        title={detailRow != null ? detailRow.msisdn : "Opt-out"}
        description={detailRow != null ? `Opt-out ${detailRow.id}` : undefined}
        footer={
          detailRow != null ? (
            <Button
              type="button"
              variant="destructive"
              size="sm"
              onClick={() => openRemoveConfirm(detailRow.id)}
            >
              Remove
            </Button>
          ) : undefined
        }
      >
        {detailRow != null && <OptOutDetailFields row={detailRow} />}
      </QuickDetailDrawer>

      <RemoveConfirmDialog
        open={urlState.remove !== null}
        pending={deleteMutation.isPending}
        onOpenChange={(open) => !open && closeRemoveConfirm()}
        onConfirm={confirmRemove}
      />
    </ScreenStack>
  );
}
