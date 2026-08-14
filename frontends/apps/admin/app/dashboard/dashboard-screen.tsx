"use client";

// The Dashboard screen (#49): throughput, delivery rate by operator, queue
// depth, outbox depth, job backlog, balance, and a UCS-2-ratio trend — the
// epic's own words: "a sudden jump means someone shipped a template with a
// ç or a smart apostrophe, and it will show up in the bill before anyone
// notices in the UI." That line is the acceptance bar this screen is held
// to, not a decoration — see the UCS-2 card below for how it's met.
//
// # Where every number comes from
//
// One call, `dashboard.summary` -> `dashboardSummary` (`backends/crates/sms-api/src/
// procedures.rs`'s own doc on `dashboard_snapshot` has the full reasoning).
// Short version: cratestack's own `aggregate()` has no GROUP BY, so the
// server runs ~26 small, policy-scoped `COUNT` queries rather than one
// grouped query, cached 15s server-side. Two Prometheus gauges this
// dashboard deliberately does NOT recompute — `sms_webhook_outbox_oldest_
// undelivered_age_seconds` and `sms_event_outbox_poison_rows`
// (`backends/crates/sms-metrics`) — describe the framework's own internal event
// outbox (drained by the `drain` role), a different table from this
// schema's own `WebhookAttempt` rows (`outboxDepth` below, delivered by
// the `hooks` role). Building a second, disagreeing "outbox" number here
// would be exactly the mistake AGENTS.md warns against; the outbox card's
// own caption says so and points at `deploy/prometheus/alerts.yml` instead
// of restating those two numbers.
//
// # Why the scope banner
//
// **Changed by #211.** Before it, this call always ran as the console's
// own machine credential, so `Message`/`WebhookAttempt` numbers were
// scoped to that credential's one fixed `appId`. #211 forwards the
// signed-in human's own session token instead, and `Message`/
// `WebhookAttempt`'s own `@@allow` (`schema.cstack`) admits
// `auth().kind == "user"` unconditionally — unscoped by `appId`, for
// *any* signed-in human regardless of role. So the numbers below now
// cover every app in this deployment, not one — a real widening of what a
// signed-in operator sees, not a bug: it is the "cross-app visibility"
// #211's own issue named as one of the things it unblocks (#50). The
// banner still renders off `summary.appId`'s presence/absence rather than
// assuming either shape, matching `messages-screen.tsx`'s own precedent.
// `jobBacklog` was always system-wide regardless, because `Job` has no
// `appId` to scope by at all (`jobs-screen.tsx`'s own banner).
//
// # Delivery rate, and what "uncertain" means for it
//
// `operatorStats[].terminalTotal` excludes `uncertain`/`undelivered` on
// purpose (`OPEN_QUESTIONS.md` §3.2 / `docs/architecture.md`: `uncertain`
// is "not a synonym for failure" — it was never confirmed either way).
// Counting it as failed overstates failure; counting it as delivered is a
// lie. `stuckMessages` reports that count on its own instead.
//
// # Balance
//
// No card claims a number here. `poll_balance` (§7.5) was never built —
// AGENTS.md's own status section names it as one of the eight job kinds
// "named but never built" — so there is no source of truth for provider
// balance anywhere in this system yet. A tile showing a fabricated "0
// XAF" would be actively misleading; the card below says exactly that
// instead.
//
// # R6
//
// This file holds data fetching and derived values only (AGENTS.md's R6:
// "pages compose, smart components decide, dumb components style") — every
// class and every piece of markup lives in `./components/*`, composed by
// `DashboardView`.

import type { inferRouterOutputs } from "@trpc/server";
import type { AppRouter } from "@vsms/api";
import { trpc } from "@vsms/hooks";
import { DashboardView } from "./components/dashboard-view";
import { formatCount } from "./format";

// Console-redesign Phase 2: this screen used to hand-roll its own <header>
// nav strip (links to every other route in the console) and its own
// <main max-w-[1400px] px-6 py-10> wrapper — both pre-date `ConsoleShell`
// (Phase 0), which now mounts once from `admin/app/layout.tsx` and already
// gives every route a persistent sidebar (`SideNav`, all eighteen routes,
// grouped) plus a shared <main> with its own max-width and padding. That
// block is gone; `DashboardView` renders only this screen's own content.

type RouterOutputs = inferRouterOutputs<AppRouter>;
type DashboardSummary = RouterOutputs["dashboard"]["summary"];

export interface DashboardScreenProps {
  /** `env.DASHBOARD_REFETCH_INTERVAL_MS`, read server-side (`page.tsx`) so
   * the value comes from `@vsms/env` in exactly one place — see that
   * file's own comment. */
  refetchIntervalMs: number;
}

export function DashboardScreen({ refetchIntervalMs }: DashboardScreenProps) {
  const summaryQuery = trpc.dashboard.summary.useQuery(undefined, {
    refetchInterval: refetchIntervalMs,
  });

  const data: DashboardSummary | undefined = summaryQuery.data;
  const buckets = data?.hourlyBuckets ?? [];
  const currentHour = buckets[buckets.length - 1];
  const previousHour = buckets[buckets.length - 2];

  const throughputDelta =
    currentHour != null && previousHour != null
      ? currentHour.totalCount - previousHour.totalCount
      : null;

  const ratioOf = (bucket: { totalCount: number; ucs2Count: number } | undefined) =>
    bucket != null && bucket.totalCount > 0 ? bucket.ucs2Count / bucket.totalCount : null;

  const currentRatio = ratioOf(currentHour);
  const previousRatio = ratioOf(previousHour);
  // A "sudden jump" (the epic's own words), not just "went up": 25
  // percentage points in one hour is the threshold — arbitrary, but
  // deliberately far enough above normal template-mix noise that it
  // wouldn't fire on, say, one op-ed's worth of French copy sprinkled
  // through an otherwise-ASCII campaign.
  const ucs2Jumped =
    currentRatio != null && previousRatio != null && currentRatio - previousRatio >= 0.25;

  const operatorRows = (data?.operatorStats ?? []).filter((row) => row.terminalTotal > 0);
  const allOperatorsQuiet = data != null && operatorRows.length === 0;
  const stuckMessagesCount = data?.stuckMessages ?? 0;

  return (
    <DashboardView
      refetchIntervalMs={refetchIntervalMs}
      appScoped={data?.appId !== undefined}
      errorMessage={summaryQuery.isError ? summaryQuery.error.message : null}
      isLoading={summaryQuery.isLoading}
      queueDepth={formatCount(data?.queueDepth ?? 0)}
      jobBacklog={formatCount(data?.jobBacklog ?? 0)}
      outboxDepth={formatCount(data?.outboxDepth ?? 0)}
      stuckMessages={formatCount(stuckMessagesCount)}
      stuckMessagesCount={stuckMessagesCount}
      buckets={buckets}
      throughputDelta={throughputDelta}
      currentRatio={currentRatio}
      previousRatio={previousRatio}
      ucs2Jumped={ucs2Jumped}
      operatorRows={operatorRows}
      allOperatorsQuiet={allOperatorsQuiet}
    />
  );
}
