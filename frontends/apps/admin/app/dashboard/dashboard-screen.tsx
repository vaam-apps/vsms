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
// own comment says so and points at `deploy/prometheus/alerts.yml` instead
// of restating those two numbers.
//
// # Scope
//
// Since #211 this call runs as the signed-in human, and `Message`/
// `WebhookAttempt`'s `@@allow` admits any `kind == "user"` unscoped, so the
// figures cover every app in the deployment. The panel's caption says so,
// read off `summary.appId`'s presence rather than assumed. `jobBacklog` is
// system-wide regardless: `Job` has no `appId` to scope by.
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
// No card. `poll_balance` (§7.5) was never built, so there is no source of
// truth for provider balance anywhere in this system; the old card said so
// in a paragraph on every load, which told an operator nothing they could
// act on. It comes back as a figure when the job exists.
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
import { describeDelta, formatCount, formatPercent, overallDeliveryRate } from "./format";

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
  const stuckMessagesCount = data?.stuckMessages ?? 0;
  const overall = overallDeliveryRate(operatorRows);
  const appScoped = data?.appId !== undefined;

  return (
    <DashboardView
      refreshSeconds={Math.round(refetchIntervalMs / 1000)}
      scopeCaption={
        appScoped
          ? "This console's own app only. Jobs are system-wide."
          : "Every app in this deployment. Jobs are system-wide."
      }
      errorMessage={summaryQuery.isError ? summaryQuery.error.message : null}
      isLoading={summaryQuery.isLoading}
      sentThisHour={{
        value: formatCount(currentHour?.totalCount ?? 0),
        caption: describeDelta(throughputDelta) ?? "No previous hour to compare",
      }}
      deliveryRate={
        overall == null
          ? { value: "—", caption: "Nothing reached a final state" }
          : {
              value: formatPercent(overall.ratio),
              caption: `${formatCount(overall.delivered)} of ${formatCount(overall.terminal)} final`,
            }
      }
      queueDepth={{
        value: formatCount(data?.queueDepth ?? 0),
        caption: "Accepted, queued or routed",
      }}
      stuck={{
        value: formatCount(stuckMessagesCount),
        caption: "Uncertain or undelivered",
        attention: stuckMessagesCount > 0,
      }}
      pipeline={[
        {
          label: "Queued messages",
          value: formatCount(data?.queueDepth ?? 0),
          caption: "Accepted, queued or routed, not yet with a provider",
          href: "/messages?state=queued",
        },
        {
          label: "Messages with no outcome",
          value: formatCount(stuckMessagesCount),
          caption: "Sent, but never confirmed delivered or failed",
          href: "/messages?state=uncertain",
        },
        {
          label: "Job backlog",
          value: formatCount(data?.jobBacklog ?? 0),
          caption: "Pending or running, across the whole system",
          href: "/jobs",
        },
        {
          label: "Webhook deliveries",
          value: formatCount(data?.outboxDepth ?? 0),
          caption: "Attempts pending or in flight",
          href: "/webhooks",
        },
      ]}
      buckets={buckets}
      currentRatio={currentRatio}
      ucs2Jumped={ucs2Jumped}
      operatorRows={operatorRows}
      stuckMessagesCount={stuckMessagesCount}
    />
  );
}
