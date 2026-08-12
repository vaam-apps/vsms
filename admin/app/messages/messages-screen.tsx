"use client";

// The messages list (T12) — filters (state, date range, `clientRef`) via
// `nuqs` URL state, and live updates via T10's polling stream hub
// (`messages.onStateChange`, `@vsms/api/routers/messages.ts`).
//
// **This is polling with a streaming interface, not streaming** — per the
// architecture plan's DECISIONS §3, permanently, not just for now. Median
// latency ≈ `pollMs`. Nothing in this file, its copy, or its behaviour
// should read as "live" in the WebSocket/SSE sense.
//
// # Why the visible single-app-scope banner
//
// The console's own gateway credential (`SMS_CONSOLE_CLIENT_ID`) belongs
// to exactly one `App`, and `Message`'s own row policy
// (`appId == auth().appId`) enforces that server-side regardless of any
// query param this screen sends — verified live, `@vsms/gateway/
// messages.ts`'s own module doc, point 9. An operator with no context for
// that would read an unfamiliar-looking, single-app list as a bug or
// missing data. The banner below says why, explicitly, rather than
// leaving it to be discovered.
//
// # Live reconciliation, briefly
//
// `rows` holds what's on screen, seeded fresh from `messages.list`
// whenever the query key (i.e. the filters) changes. `messages.
// onStateChange` is polled continuously (its own bounded long-poll on the
// server keeps each individual call short) and DELIBERATELY subscribes to
// **every** state, not just whatever this screen's own state filter
// currently shows — narrowing it would mean a message transitioning OUT
// of the filtered state (e.g. `queued` → `routed` while filtered to
// `queued`) would never arrive, and the row would sit frozen on screen
// showing a state it no longer has. Filtering happens here, in
// `applyEvent`, against the *current* filter, on every incoming event.
//
// Design doc §6.5's live-list rules, as implemented:
// 1. The list never auto-scrolls — inserting a buffered row scrolls
//    `window` explicitly, only on a click.
// 2. At scroll-top (`window.scrollY <= 8`), new rows insert directly with
//    a wash; scrolled away, they buffer behind the "N new" pill.
// 3. In-place status change never moves a row — `applyEvent` always
//    `.map()`s the existing array, never removes/reinserts on update. The
//    default sort is `-createdAt`, which never changes for an existing
//    row, so this is always safe (this screen offers no status-sort
//    control, so rule 3's "switch to fully-buffered mode" branch doesn't
//    apply here).
// 6. Row identity is the message id — `key={row.id}` throughout, `LiveRow`
//    itself requires a stable identity to wash correctly.
// 8. Connection loss is visible — a `degraded` frame flips an inline bar
//    on; a `recovered` frame flips it off.

import type { inferRouterOutputs } from "@trpc/server";
// Type-only — see this file's own note below. `admin` already depends on
// `@vsms/api` for its route handler
// (`app/api/trpc/[trpc]/route.ts`); this is a second, purely type-level
// use of that same dependency, erased at build time
// (`verbatimModuleSyntax`), not a new runtime import of the server router.
import type { AppRouter } from "@vsms/api";
import { trpc } from "@vsms/hooks";
import {
  Button,
  IdDisplay,
  InlineEmptyState,
  Input,
  Label,
  LiveRow,
  MESSAGE_STATES,
  MESSAGE_STATUS_META,
  type MessageState,
  MsisdnDisplay,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  Skeleton,
  StatusPill,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
  ThemeToggle,
  TimestampDisplay,
} from "@vsms/ui";
import { parseAsString, parseAsStringEnum, useQueryStates } from "nuqs";
import { useEffect, useMemo, useRef, useState } from "react";

// --- Types mirroring the tRPC procedures' own inferred shapes. ----------
// (Not imported from `@vsms/gateway` directly — this component only ever
// needs what the router itself infers, via `@trpc/server`'s
// `inferRouterOutputs`, type-only throughout.)

type RouterOutputs = inferRouterOutputs<AppRouter>;
type MessageListItem = RouterOutputs["messages"]["list"]["items"][number];
type StreamFrame = RouterOutputs["messages"]["onStateChange"]["frames"][number];

const STATE_LABELS: Record<MessageState, string> = Object.fromEntries(
  MESSAGE_STATES.map((state) => [state, MESSAGE_STATUS_META[state].labelEn]),
) as Record<MessageState, string>;

function todayIsoDate(): string {
  return new Date().toISOString().slice(0, 10);
}

function daysAgoIsoDate(days: number): string {
  const date = new Date();
  date.setUTCDate(date.getUTCDate() - days);
  return date.toISOString().slice(0, 10);
}

/** `to` in `messages.list`'s input is exclusive (`@vsms/gateway/
 * messages.ts`'s own doc) — a date-only picker selecting "2026-08-08"
 * should include the whole day, so this steps one day past it. */
function nextDayIso(dateOnly: string): string {
  const date = new Date(`${dateOnly}T00:00:00.000Z`);
  date.setUTCDate(date.getUTCDate() + 1);
  return date.toISOString();
}

interface ReconcileState {
  rows: MessageListItem[];
  pending: MessageListItem[];
}

/** Merges one live state-change event into the current view — see this
 * file's own module doc for the reconciliation rules. `null` `stateFilter`
 * means "no state filter active," matching every event. */
function applyEvent(
  prev: ReconcileState,
  event: Extract<StreamFrame, { type: "message" }>,
  stateFilter: MessageState | null,
  scrolledAway: boolean,
): ReconcileState {
  const matchesFilter = stateFilter === null || event.state === stateFilter;
  const inRows = prev.rows.some((row) => row.id === event.id);
  const inPending = prev.pending.some((row) => row.id === event.id);

  function merge(row: MessageListItem): MessageListItem {
    return {
      ...row,
      state: event.state,
      stateReason: event.stateReason ?? undefined,
      providerMessageRef: event.providerMessageRef ?? undefined,
      version: event.version,
      updatedAt: event.occurredAt,
    };
  }

  if (!matchesFilter) {
    // No longer belongs in this filtered view — drop it if it was here.
    if (!inRows && !inPending) return prev;
    return {
      rows: prev.rows.filter((row) => row.id !== event.id),
      pending: prev.pending.filter((row) => row.id !== event.id),
    };
  }

  if (inRows) {
    return { ...prev, rows: prev.rows.map((row) => (row.id === event.id ? merge(row) : row)) };
  }
  if (inPending) {
    return {
      ...prev,
      pending: prev.pending.map((row) => (row.id === event.id ? merge(row) : row)),
    };
  }

  // A genuinely new row for this view. `MessageListItem` doesn't carry
  // every field a stream event lacks (msisdn, clientRef, senderIdValue,
  // encoding) — those are populated as best-effort empty/placeholder
  // values until the next full `messages.list` refetch (a filter change)
  // fills them in properly. `createdAt` is approximated from the event's
  // own timestamp since the stream doesn't carry it either; harmless here
  // because the default sort is insertion-order (new rows always join at
  // the top), never re-derived from this value.
  const placeholder: MessageListItem = {
    id: event.id,
    appId: event.appId,
    msisdn: "",
    operator: event.operator,
    senderIdValue: "",
    class: "transactional",
    state: event.state,
    stateReason: event.stateReason ?? undefined,
    encoding: "gsm7",
    segments: event.segments,
    providerMessageRef: event.providerMessageRef ?? undefined,
    version: event.version,
    createdAt: event.occurredAt,
    updatedAt: event.occurredAt,
  };

  if (scrolledAway) {
    return { ...prev, pending: [placeholder, ...prev.pending] };
  }
  return { ...prev, rows: [placeholder, ...prev.rows] };
}

export interface MessagesScreenProps {
  /** `MESSAGE_STREAM_POLL_MS`, read server-side (`page.tsx`) so the
   * browser's own poll cadence stays in lockstep without a duplicate
   * `NEXT_PUBLIC_*` env var. */
  pollMs: number;
}

export function MessagesScreen({ pollMs }: MessagesScreenProps) {
  const [filters, setFilters] = useQueryStates(
    {
      state: parseAsStringEnum<MessageState>([...MESSAGE_STATES]),
      clientRef: parseAsString,
      from: parseAsString,
      to: parseAsString,
    },
    { history: "push" },
  );

  const listInput = useMemo(
    () => ({
      state: filters.state ?? undefined,
      clientRef: filters.clientRef ?? undefined,
      from: filters.from ? `${filters.from}T00:00:00.000Z` : undefined,
      to: filters.to ? nextDayIso(filters.to) : undefined,
      limit: 100,
      sort: "-createdAt" as const,
    }),
    [filters],
  );

  const listQuery = trpc.messages.list.useQuery(listInput);
  const utils = trpc.useUtils();

  const [rows, setRows] = useState<MessageListItem[]>([]);
  const [pending, setPending] = useState<MessageListItem[]>([]);
  const [degraded, setDegraded] = useState(false);
  const [scrolledAway, setScrolledAway] = useState(false);

  // Read inside the stream-reconciliation effect below via `.current`
  // rather than as effect dependencies — that effect must run exactly
  // once per incoming batch of stream frames (`streamQuery.data`
  // changing), never re-run just because scroll position or the pending
  // buffer changed in between, while still always seeing their *current*
  // value rather than a stale one captured at the last time the effect
  // itself re-ran. Kept in sync on every render (no effect needed for a
  // synchronous ref assignment).
  const pendingRef = useRef(pending);
  pendingRef.current = pending;
  const stateFilterRef = useRef(filters.state);
  stateFilterRef.current = filters.state;
  const scrolledAwayRef = useRef(scrolledAway);
  scrolledAwayRef.current = scrolledAway;

  // Seed (or reseed, on a filter change) fresh from the authoritative list
  // fetch. Live events layer on top from here.
  useEffect(() => {
    if (listQuery.data !== undefined) {
      setRows(listQuery.data.items);
      setPending([]);
    }
  }, [listQuery.data]);

  useEffect(() => {
    function onScroll() {
      setScrolledAway(window.scrollY > 8);
    }
    window.addEventListener("scroll", onScroll, { passive: true });
    return () => window.removeEventListener("scroll", onScroll);
  }, []);

  // Drives `messages.onStateChange` itself, deliberately NOT via
  // `useQuery({ refetchInterval })`. Each call is already a bounded
  // server-side long-poll (up to `pollMs`) — chaining `refetchInterval`
  // on top of that ties the poll loop's liveness to React Query's observer
  // lifecycle (which this component's own frequent re-renders, from
  // `setRows`/`setPending`/`setDegraded`, churn constantly), and that
  // combination was found live to stall the loop after its first one or
  // two calls rather than continuing indefinitely — every individual
  // request still resolved correctly (confirmed with a raw `fetch()` and
  // by calling the procedure directly), but no *further* request was ever
  // issued. `utils.client.messages.onStateChange.query(...)` is the raw
  // vanilla client: a plain imperative call with no cache subscription and
  // nothing tying its next invocation to a render. This self-schedules its
  // own next call only after the current one settles, which is exactly
  // "one poll in flight at a time" without depending on React Query's own
  // scheduler to get that right for a long-poll shaped procedure.
  //
  // Mount-once by design (`trpc.useUtils()` is stable across renders): the
  // loop reads current filter/scroll state via the refs above, not via
  // closures that would need this effect to re-run when they change.
  // biome-ignore lint/correctness/useExhaustiveDependencies: mount-once by design, see comment above
  useEffect(() => {
    let cancelled = false;

    async function loop() {
      while (!cancelled) {
        try {
          const result = await utils.client.messages.onStateChange.query({});
          if (cancelled) return;

          for (const frame of result.frames) {
            if (frame.type === "degraded") setDegraded(true);
            else if (frame.type === "recovered") setDegraded(false);
          }

          const messageEvents = result.frames.filter(
            (frame): frame is Extract<StreamFrame, { type: "message" }> => frame.type === "message",
          );
          if (messageEvents.length > 0) {
            setRows((prevRows) => {
              let state: ReconcileState = { rows: prevRows, pending: pendingRef.current };
              for (const event of messageEvents) {
                state = applyEvent(state, event, stateFilterRef.current, scrolledAwayRef.current);
              }
              if (state.pending !== pendingRef.current) setPending(state.pending);
              return state.rows;
            });
          }
        } catch {
          // A failure reaching this console's OWN Next.js server (not
          // sms-api — the hub's own upstream failures surface as
          // `degraded` frames inside a successful response, handled
          // above). Brief backoff before the loop retries on its own.
          if (cancelled) return;
          setDegraded(true);
          await new Promise((resolve) => setTimeout(resolve, 2000));
        }
      }
    }

    void loop();
    return () => {
      cancelled = true;
    };
  }, []);

  function insertPending() {
    setRows((prev) => [...pending, ...prev]);
    setPending([]);
    window.scrollTo({ top: 0, behavior: "smooth" });
  }

  function clearFilters() {
    void setFilters({ state: null, clientRef: null, from: null, to: null });
  }

  const hasFilters =
    filters.state !== null ||
    (filters.clientRef ?? "") !== "" ||
    filters.from !== null ||
    filters.to !== null;

  return (
    <main className="mx-auto flex max-w-[1400px] flex-col gap-6 px-6 py-10">
      <header className="flex items-start justify-between gap-4 border-edge border-b pb-6">
        <div>
          <p className="font-mono text-micro text-subtle-foreground tracking-[0.03em]">
            vsms admin console
          </p>
          <h1 className="mt-1 font-medium text-foreground text-title">Messages</h1>
          <p className="mt-1 max-w-xl text-body text-muted-foreground">
            Live status of every message this app has sent — polled every ~
            {Math.round(pollMs / 1000)}s, not pushed. New rows while you're scrolled down buffer
            behind a pill rather than jumping the list.
          </p>
        </div>
        <div className="flex shrink-0 items-center gap-3">
          <a
            href="/"
            className="text-caption text-muted-foreground underline decoration-edge-strong underline-offset-2 hover:decoration-foreground"
          >
            Composer
          </a>
          <a
            href="/jobs"
            className="text-caption text-muted-foreground underline decoration-edge-strong underline-offset-2 hover:decoration-foreground"
          >
            Jobs
          </a>
          <a
            href="/workers"
            className="text-caption text-muted-foreground underline decoration-edge-strong underline-offset-2 hover:decoration-foreground"
          >
            Workers
          </a>
          <a
            href="/gallery"
            className="text-caption text-muted-foreground underline decoration-edge-strong underline-offset-2 hover:decoration-foreground"
          >
            Component gallery
          </a>
          <ThemeToggle />
        </div>
      </header>

      <div className="rounded-sm border border-edge bg-surface-2 px-3 py-2 text-caption text-muted-foreground">
        Scoped to <span className="font-mono text-foreground">this app only</span> — the console's
        own service-account token can only read the one app it belongs to, so there is nothing to
        switch to. This is not a filter and not a bug; see the architecture plan's DECISIONS §1/§2.
      </div>

      {degraded && (
        <div className="rounded-sm border border-state-uncertain-border bg-state-uncertain-bg px-3 py-2 text-caption text-state-uncertain-fg">
          Live updates paused — reconnecting.
        </div>
      )}

      <div className="flex flex-wrap items-end gap-4">
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="filter-state">State</Label>
          <Select
            value={filters.state ?? "__all"}
            onValueChange={(value) =>
              void setFilters({ state: value === "__all" ? null : (value as MessageState) })
            }
          >
            <SelectTrigger id="filter-state" className="w-[180px]">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="__all">All states</SelectItem>
              {MESSAGE_STATES.map((state) => (
                <SelectItem key={state} value={state}>
                  {STATE_LABELS[state]}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>

        <div className="flex flex-col gap-1.5">
          <Label htmlFor="filter-client-ref">Client reference</Label>
          <Input
            id="filter-client-ref"
            placeholder="exact match"
            className="w-[200px]"
            value={filters.clientRef ?? ""}
            onChange={(e) =>
              void setFilters({ clientRef: e.target.value === "" ? null : e.target.value })
            }
          />
        </div>

        <div className="flex flex-col gap-1.5">
          <Label htmlFor="filter-from">From</Label>
          <Input
            id="filter-from"
            type="date"
            className="w-[160px]"
            value={filters.from ?? ""}
            max={filters.to ?? undefined}
            onChange={(e) =>
              void setFilters({ from: e.target.value === "" ? null : e.target.value })
            }
          />
        </div>
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="filter-to">To</Label>
          <Input
            id="filter-to"
            type="date"
            className="w-[160px]"
            value={filters.to ?? ""}
            min={filters.from ?? undefined}
            onChange={(e) => void setFilters({ to: e.target.value === "" ? null : e.target.value })}
          />
        </div>

        <div className="flex items-center gap-2 pb-0.5">
          <Button
            type="button"
            variant="secondary"
            size="sm"
            onClick={() => void setFilters({ from: todayIsoDate(), to: todayIsoDate() })}
          >
            Today
          </Button>
          <Button
            type="button"
            variant="secondary"
            size="sm"
            onClick={() => void setFilters({ from: daysAgoIsoDate(7), to: todayIsoDate() })}
          >
            Last 7 days
          </Button>
          <Button
            type="button"
            variant="secondary"
            size="sm"
            onClick={() => void setFilters({ from: daysAgoIsoDate(30), to: todayIsoDate() })}
          >
            Last 30 days
          </Button>
          {hasFilters && (
            <Button type="button" variant="ghost" size="sm" onClick={clearFilters}>
              Clear filters
            </Button>
          )}
        </div>
      </div>

      {listQuery.data?.truncated && (
        <p className="text-caption text-subtle-foreground">
          Showing the most recent 1000 messages for this app — sms-api's `GET /messages` has no
          server-side filter for state or date range (see `@vsms/gateway/messages.ts`'s module doc),
          so filtering happens over that window. Older matches outside it won't appear.
        </p>
      )}

      <div className="relative">
        {pending.length > 0 && (
          <div className="-translate-x-1/2 sticky top-2 left-1/2 z-20 flex w-fit justify-center">
            <button
              type="button"
              onClick={insertPending}
              className="rounded-full border border-edge bg-surface-2 px-3 py-1 text-caption text-foreground shadow-[var(--shadow-popover)] duration-[var(--dur-enter)] ease-out"
            >
              {pending.length} new message{pending.length === 1 ? "" : "s"}
            </button>
          </div>
        )}

        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Status</TableHead>
              <TableHead>Recipient</TableHead>
              <TableHead>Client ref</TableHead>
              <TableHead>Sender</TableHead>
              <TableHead>Encoding</TableHead>
              <TableHead>Id</TableHead>
              <TableHead align="end">Time</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {listQuery.isLoading &&
              Array.from({ length: 8 }).map((_, i) => (
                // biome-ignore lint/suspicious/noArrayIndexKey: static skeleton rows, never reordered or diffed
                <TableRow key={i}>
                  <TableCell colSpan={7}>
                    <Skeleton className="h-4 w-full" />
                  </TableCell>
                </TableRow>
              ))}

            {!listQuery.isLoading && rows.length === 0 && (
              <tr>
                <td colSpan={7}>
                  <InlineEmptyState
                    message={
                      hasFilters
                        ? "No messages match the current filters."
                        : "No messages yet for this app."
                    }
                    {...(hasFilters
                      ? { action: { label: "Clear filters", onClick: clearFilters } }
                      : {})}
                  />
                </td>
              </tr>
            )}

            {rows.map((row) => (
              <LiveRow
                key={row.id}
                washTrigger={row.version}
                washHue={MESSAGE_STATUS_META[row.state].hue}
              >
                <TableCell>
                  <StatusPill state={row.state} />
                </TableCell>
                <TableCell>
                  <MsisdnDisplay value={row.msisdn} operator={row.operator} />
                </TableCell>
                <TableCell mono>{row.clientRef ?? "—"}</TableCell>
                <TableCell mono>{row.senderIdValue}</TableCell>
                <TableCell mono>
                  {row.encoding.toUpperCase()} · {row.segments}
                </TableCell>
                <TableCell>
                  <div className="flex items-center gap-2">
                    <IdDisplay value={row.id} />
                    {/* #50: the detail route. A plain `<a>`, matching every
                     * other internal nav link on this screen (Composer/
                     * Jobs/Workers/Gallery, in the header) — not `next/
                     * link`'s `Link`. Separate from `IdDisplay` itself
                     * rather than wrapping it: `IdDisplay`'s own copy
                     * button doesn't stop propagation, so wrapping it in
                     * an `<a>` would fire a navigation on every copy
                     * click. */}
                    <a
                      href={`/messages/${row.id}`}
                      className="text-caption text-muted-foreground underline decoration-edge-strong underline-offset-2 hover:decoration-foreground"
                    >
                      View
                    </a>
                  </div>
                </TableCell>
                <TableCell align="end">
                  <TimestampDisplay value={row.createdAt} />
                </TableCell>
              </LiveRow>
            ))}
          </TableBody>
        </Table>
      </div>
    </main>
  );
}
