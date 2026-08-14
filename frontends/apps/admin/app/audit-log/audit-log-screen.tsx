"use client";

// The Audit log screen (#58): a filtered, paged window over
// `cratestack_audit`, plus the anchor hash chain's own current status —
// "does this period's chain verify," not just a table dump. See
// `backends/crates/sms-api/src/audit_log.rs`'s own module doc for the full
// mechanism this reads.
//
// # Console redesign (Phase 2, Admin group) — what changed and why
//
// This screen stays a **`Page`**, unchanged
// (docs/design/console-redesign.md §3: "a page-scale table, not a record
// of something else"), but a row can now be opened as a
// **`QuickDetailDrawer`** — a peek at one audited write's `before`/`after`
// values without leaving the filtered table underneath, matching the
// Mercury reference (§1.4) this rule is drawn from. Deliberately *not* a
// `MoreDetailDrawer`: there is no edit form to grow into (see this file's
// own "genuinely read-only" note below), and no route ownership either —
// D14 reserves that for drawers a caller needs to survive a refresh or
// share a link to, and re-opening the same row after a refresh is one
// click on the same table. `model`/`operation`/`actorId`/`since`/`until`
// **are** now `nuqs` URL state (`history: "push"`, matching
// `messages-screen.tsx`/`jobs-screen.tsx`'s own filter convention) —
// unlike the row drawer, a filtered *search* over the audit log is exactly
// the kind of thing worth bookmarking or sending to a teammate. `offset`
// stays URL state too, but `history: "replace"` — paging through one
// search isn't a new view worth a distinct back-button stop the way
// changing the search itself is.
//
// # Genuinely read-only — no write path exists anywhere on this screen
//
// There is no edit/delete action anywhere below because there is nothing
// to call: `backends/crates/sms-api/src/procedures.rs` never constructs an
// `AuditAnchor` update or delete, and — checked live against a real
// Postgres, not assumed — no role, human or synthetic, can write one
// through any path this codebase exposes at all (`audit_log.rs`'s own
// module doc has the exact runtime error captured). This screen's own copy
// says so rather than leaving "why is there no edit button" to be
// inferred, and the row drawer below is read-only for the identical
// reason — a peek at evidence, never an editor for it.
//
// # R6 — layer split, and one fix made while doing it
//
// All markup and classes now live in `./components/*`; this file is data
// fetching, URL state, and derived values only. One real bug fixed along
// the way, not just moved: the previous version held the *entire* selected
// `AuditLogEntry` object in local `useState` — exactly the `detailRow`
// anti-pattern R6 names explicitly ("a copy of a server object... a second
// source of truth"). It only avoided a stale-fallback expression because
// audit entries are immutable, but it was still two sources of truth for
// the same row. Fixed by keeping only the row's own `eventId` in state and
// deriving the entry from `listQuery.data` — the same "id in, look it up"
// shape R6 recommends for the URL-state case, applied here to plain local
// state since this particular drawer deliberately isn't URL-backed (see
// the note above). The row stays resolvable because it can only ever be
// selected from the page that's currently loaded.
//
// `LIMIT` (the page size) moved to `@vsms/env`'s `AUDIT_LOG_PAGE_SIZE` —
// an operational tuning value, not a protocol constant, the same test
// `MESSAGE_STREAM_POLL_MS` already sets a precedent for — read server-side
// in `page.tsx` and passed down as `pageSize`.

import { trpc } from "@vsms/hooks";
import { ScreenStack } from "@vsms/ui";
import { parseAsInteger, parseAsString, parseAsStringEnum, useQueryStates } from "nuqs";
import { useEffect, useState } from "react";
import { AuditEntryDrawer } from "./components/audit-entry-drawer";
import { AuditFilters } from "./components/audit-filters";
import { AuditLogHeader } from "./components/audit-log-header";
import { AuditLogPagination } from "./components/audit-log-pagination";
import { AuditLogTable } from "./components/audit-log-table";
import { ChainStatusPanel, type ChainStatusPanelProps } from "./components/chain-status-panel";
import { type AuditOperation, OPERATIONS } from "./types";

function ChainStatusBanner() {
  const statusQuery = trpc.auditLog.chainStatus.useQuery();

  let props: ChainStatusPanelProps;
  if (statusQuery.isLoading) {
    props = { kind: "loading" };
  } else if (statusQuery.isError) {
    props = { kind: "error", message: statusQuery.error.message };
  } else if (statusQuery.data === undefined || statusQuery.data.latestAnchorId === undefined) {
    props = { kind: "no-anchor" };
  } else {
    const status = statusQuery.data;
    const broken = status.linkageBreaks.length > 0 || status.latestContentVerified === false;
    props = broken
      ? {
          kind: "broken",
          linkageBreakCount: status.linkageBreaks.length,
          contentVerified: status.latestContentVerified,
        }
      : { kind: "ok", rowCount: status.latestRowCount ?? 0, periodEnd: status.latestPeriodEnd };
  }

  return <ChainStatusPanel {...props} />;
}

export function AuditLogScreen({ pageSize }: { pageSize: number }) {
  const [filters, setFilters] = useQueryStates(
    {
      model: parseAsString,
      operation: parseAsStringEnum<AuditOperation>([...OPERATIONS]),
      actorId: parseAsString,
      since: parseAsString,
      until: parseAsString,
    },
    { history: "push" },
  );
  // Paging through one search isn't a new view worth its own back-button
  // stop the way changing the search is — see this file's own module doc.
  const [offset, setOffset] = useQueryStates(
    { offset: parseAsInteger.withDefault(0) },
    { history: "replace" },
  );

  // Only the id is kept in state — see this file's own module doc for why
  // that replaces the earlier full-object `useState`.
  const [selectedEntryId, setSelectedEntryId] = useState<string | null>(null);
  // See `apps-screen.tsx`'s own `stickyPanelId` doc — the drawer stays
  // mounted below so its `vaul` close transition can play.
  const [stickyEntryId, setStickyEntryId] = useState<string | null>(null);
  useEffect(() => {
    if (selectedEntryId !== null) setStickyEntryId(selectedEntryId);
  }, [selectedEntryId]);

  const listQuery = trpc.auditLog.list.useQuery({
    model: filters.model ?? undefined,
    operation: filters.operation ?? undefined,
    actorId: filters.actorId ?? undefined,
    since: filters.since ? `${filters.since}T00:00:00.000Z` : undefined,
    until: filters.until ? `${filters.until}T23:59:59.999Z` : undefined,
    limit: pageSize,
    offset: offset.offset,
  });
  const entries = listQuery.data?.entries ?? [];
  const stickyEntry = entries.find((entry) => entry.eventId === stickyEntryId) ?? null;

  const hasFilters =
    (filters.model ?? "") !== "" ||
    filters.operation !== null ||
    (filters.actorId ?? "") !== "" ||
    (filters.since ?? "") !== "" ||
    (filters.until ?? "") !== "";

  function clearFilters() {
    void setFilters({ model: null, operation: null, actorId: null, since: null, until: null });
    void setOffset({ offset: 0 });
  }

  function resetToFirstPage() {
    void setOffset({ offset: 0 });
  }

  return (
    <ScreenStack>
      <AuditLogHeader />

      <ChainStatusBanner />

      <AuditFilters
        model={filters.model ?? ""}
        operation={filters.operation}
        actorId={filters.actorId ?? ""}
        since={filters.since ?? ""}
        until={filters.until ?? ""}
        hasFilters={hasFilters}
        onModelChange={(value) => {
          void setFilters({ model: value === "" ? null : value });
          resetToFirstPage();
        }}
        onOperationChange={(value) => {
          void setFilters({ operation: value });
          resetToFirstPage();
        }}
        onActorIdChange={(value) => {
          void setFilters({ actorId: value === "" ? null : value });
          resetToFirstPage();
        }}
        onSinceChange={(value) => {
          void setFilters({ since: value === "" ? null : value });
          resetToFirstPage();
        }}
        onUntilChange={(value) => {
          void setFilters({ until: value === "" ? null : value });
          resetToFirstPage();
        }}
        onClear={clearFilters}
      />

      <AuditLogTable
        entries={entries}
        isLoading={listQuery.isLoading}
        errorMessage={listQuery.isError ? listQuery.error.message : null}
        onRowClick={(entry) => setSelectedEntryId(entry.eventId)}
      />

      <AuditLogPagination
        shownCount={entries.length}
        offset={offset.offset}
        hasMore={listQuery.data?.hasMore === true}
        onPrevious={() => void setOffset({ offset: Math.max(0, offset.offset - pageSize) })}
        onNext={() => void setOffset({ offset: offset.offset + pageSize })}
      />

      <AuditEntryDrawer
        entry={stickyEntry}
        open={selectedEntryId !== null}
        onClose={() => setSelectedEntryId(null)}
      />
    </ScreenStack>
  );
}
