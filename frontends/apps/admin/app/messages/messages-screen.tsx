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
// # Why the visible scope banner, and a real inconsistency #211 introduces
//
// **Before #211**, every call this screen made ran as the console's own
// gateway credential (`SMS_CONSOLE_CLIENT_ID`), which belongs to exactly
// one `App` — `Message`'s own row policy (`appId == auth().appId`)
// enforced a single-app list server-side regardless of any query param
// this screen sends (verified live, `@vsms/gateway/messages.ts`'s own
// module doc, point 9).
//
// **#211 changes this for the initial list/detail fetch, but deliberately
// NOT for live updates**, and that split is worth understanding rather
// than assuming away: `messages.list`/`messages.get` now run as the
// signed-in human (`@vsms/gateway/messages.ts::listMessages`/
// `getMessageById`), and `Message.list`/`.detail`'s own `@@allow`
// (`schema.cstack`) admits `auth().kind == "user"` unconditionally — so
// for any signed-in human, regardless of role, the fetched window now
// spans every app in this deployment, not one. `messages.onStateChange`
// (the live-update poll below), by contrast, is backed by
// `MessageStreamHub` — a process-wide singleton shared by every open
// browser tab, not scoped to any one request — and deliberately keeps
// using the console's own machine credential (`@vsms/gateway/
// messages.ts::listMessagesForStream`, `@vsms/gateway/request-
// credential.ts`'s own module doc explains why: there is no single human
// to attribute a shared background poll to, and forwarding whichever
// operator happened to open the first tab would leak that operator's
// credential into every other tab's stream).
//
// The practical consequence: a row belonging to an app other than the
// console's own machine credential's app can appear in the initial list,
// but will never receive a live state-change update from the stream — it
// sits on screen until the next full refetch. Accepted, not silently
// shipped: `CrossAppScopeBanner` states the list's own scope so an
// operator doesn't misread a frozen out-of-scope row as a bug.
//
// # Live reconciliation, briefly
//
// `state.rows` (below) holds what's on screen, seeded fresh from
// `messages.list` whenever the query key (i.e. the filters) changes.
// `messages.onStateChange` is polled continuously (its own bounded
// long-poll on the server keeps each individual call short) and
// DELIBERATELY subscribes to **every** state, not just whatever this
// screen's own state filter currently shows — narrowing it would mean a
// message transitioning OUT of the filtered state (e.g. `queued` →
// `routed` while filtered to `queued`) would never arrive, and the row
// would sit frozen on screen showing a state it no longer has. Filtering
// happens in `apply-event.ts`'s `applyEvent`, against the *current*
// filter, on every incoming event.
//
// Design doc §6.5's live-list rules, as implemented — the merge mechanics
// themselves live in `apply-event.ts` (extracted, tested there per R6);
// this file only wires them up:
// 1. The list never auto-scrolls — inserting a buffered row scrolls
//    `window` explicitly, only on a click (`insertPending` below).
// 2. At scroll-top (`window.scrollY <= 8`), new rows insert directly with
//    a wash; scrolled away, they buffer behind the "N new" pill.
// 3. In-place status change never moves a row — `applyEvent` always
//    `.map()`s the existing array, never removes/reinserts on update. The
//    default sort is `-createdAt`, which never changes for an existing
//    row, so this is always safe (this screen offers no status-sort
//    control, so rule 3's "switch to fully-buffered mode" branch doesn't
//    apply here).
// 6. Row identity is the message id — `key={row.id}` throughout
//    (`MessagesTable`), `LiveRow` itself requires a stable identity to
//    wash correctly.
// 8. Connection loss is visible — a `degraded` frame flips `DegradedBanner`
//    on; a `recovered` frame flips it off.

import { trpc } from "@vsms/hooks";
import { MESSAGE_STATES, type MessageState, ScreenStack } from "@vsms/ui";
import { parseAsString, parseAsStringEnum, useQueryStates } from "nuqs";
import { useEffect, useMemo, useReducer, useRef, useState } from "react";
import { CrossAppScopeBanner } from "./components/cross-app-scope-banner";
import { DegradedBanner } from "./components/degraded-banner";
import { ListErrorBanner } from "./components/list-error-banner";
import { MessagesFilters } from "./components/messages-filters";
import { MessagesHeader } from "./components/messages-header";
import { MessagesListPanel } from "./components/messages-list-panel";
import { MessagesTable } from "./components/messages-table";
import { PendingMessagesPill } from "./components/pending-messages-pill";
import { TruncatedNotice } from "./components/truncated-notice";
import { daysAgoIsoDate, nextDayIso, todayIsoDate } from "./date-range";
import { INITIAL_RECONCILE_STATE, reconcileReducer } from "./reconcile-reducer";

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

  const [state, dispatch] = useReducer(reconcileReducer, INITIAL_RECONCILE_STATE);
  // `degraded` toggles a banner, so it has to trigger a render — none of
  // the usual `useState` replacements fit it (it's neither URL state, nor
  // server data from a query, nor form state, nor several values changing
  // together), so it stays `useState` deliberately, the one case R6 itself
  // carves out an exception for.
  const [degraded, setDegraded] = useState(false);

  // Read inside the stream-reconciliation effect below via `.current`
  // rather than as effect dependencies — that effect must run exactly
  // once per incoming batch of stream frames (`messages.onStateChange`
  // resolving), never re-run just because the filter or scroll position
  // changed in between, while still always seeing their *current* value
  // rather than a stale one captured at the last time the effect itself
  // re-ran.
  const stateFilterRef = useRef(filters.state);
  stateFilterRef.current = filters.state;
  // Never read by JSX — only by the imperative loop below — so this is a
  // plain ref, not `useState`: a scroll event that only updates a ref must
  // not trigger a render.
  const scrolledAwayRef = useRef(false);

  // Seed (or reseed, on a filter change) fresh from the authoritative list
  // fetch. Live events layer on top from here.
  useEffect(() => {
    if (listQuery.data !== undefined) {
      dispatch({ type: "reset", items: listQuery.data.items });
    }
  }, [listQuery.data]);

  useEffect(() => {
    function onScroll() {
      scrolledAwayRef.current = window.scrollY > 8;
    }
    window.addEventListener("scroll", onScroll, { passive: true });
    return () => window.removeEventListener("scroll", onScroll);
  }, []);

  // Drives `messages.onStateChange` itself, deliberately NOT via
  // `useQuery({ refetchInterval })`. Each call is already a bounded
  // server-side long-poll (up to `pollMs`) — chaining `refetchInterval`
  // on top of that ties the poll loop's liveness to React Query's observer
  // lifecycle (which this component's own frequent re-renders churn
  // constantly), and that combination was found live to stall the loop
  // after its first one or two calls rather than continuing indefinitely
  // — every individual request still resolved correctly (confirmed with a
  // raw `fetch()` and by calling the procedure directly), but no *further*
  // request was ever issued. `utils.client.messages.onStateChange.query(...)`
  // is the raw vanilla client: a plain imperative call with no cache
  // subscription and nothing tying its next invocation to a render. This
  // self-schedules its own next call only after the current one settles,
  // which is exactly "one poll in flight at a time" without depending on
  // React Query's own scheduler to get that right for a long-poll shaped
  // procedure.
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
            else if (frame.type === "message") {
              dispatch({
                type: "event",
                event: frame,
                stateFilter: stateFilterRef.current,
                scrolledAway: scrolledAwayRef.current,
              });
            }
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
    dispatch({ type: "insertPending" });
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
    <ScreenStack>
      <MessagesHeader pollMs={pollMs} />
      <CrossAppScopeBanner />
      {degraded && <DegradedBanner />}
      {listQuery.isError && <ListErrorBanner message={listQuery.error.message} />}

      <MessagesFilters
        state={filters.state}
        clientRef={filters.clientRef ?? ""}
        from={filters.from ?? ""}
        to={filters.to ?? ""}
        hasFilters={hasFilters}
        onStateChange={(next) => void setFilters({ state: next })}
        onClientRefChange={(value) => void setFilters({ clientRef: value === "" ? null : value })}
        onFromChange={(value) => void setFilters({ from: value === "" ? null : value })}
        onToChange={(value) => void setFilters({ to: value === "" ? null : value })}
        onSelectToday={() => void setFilters({ from: todayIsoDate(), to: todayIsoDate() })}
        onSelectLast7Days={() => void setFilters({ from: daysAgoIsoDate(7), to: todayIsoDate() })}
        onSelectLast30Days={() => void setFilters({ from: daysAgoIsoDate(30), to: todayIsoDate() })}
        onClear={clearFilters}
      />

      {listQuery.data?.truncated && <TruncatedNotice />}

      <MessagesListPanel>
        <PendingMessagesPill count={state.pending.length} onClick={insertPending} />
        <MessagesTable
          rows={state.rows}
          isLoading={listQuery.isLoading}
          hasFilters={hasFilters}
          onClearFilters={clearFilters}
        />
      </MessagesListPanel>
    </ScreenStack>
  );
}
