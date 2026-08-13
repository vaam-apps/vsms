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
// One call, `dashboard.summary` -> `dashboardSummary` (`crates/sms-api/src/
// procedures.rs`'s own doc on `dashboard_snapshot` has the full reasoning).
// Short version: cratestack's own `aggregate()` has no GROUP BY, so the
// server runs ~26 small, policy-scoped `COUNT` queries rather than one
// grouped query, cached 15s server-side. Two Prometheus gauges this
// dashboard deliberately does NOT recompute — `sms_webhook_outbox_oldest_
// undelivered_age_seconds` and `sms_event_outbox_poison_rows`
// (`crates/sms-metrics`) — describe the framework's own internal event
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

import type { inferRouterOutputs } from "@trpc/server";
import type { AppRouter } from "@vsms/api";
import { trpc } from "@vsms/hooks";
import { Card, CardBody, CardHeader, Skeleton, ThemeToggle } from "@vsms/ui";
import { HourlyBars } from "./hourly-bars";

type RouterOutputs = inferRouterOutputs<AppRouter>;
type DashboardSummary = RouterOutputs["dashboard"]["summary"];
type OperatorStat = DashboardSummary["operatorStats"][number];

const REFETCH_INTERVAL_MS = 15_000;

const OPERATOR_LABELS: Record<OperatorStat["operator"], string> = {
  mtn: "MTN",
  orange: "Orange",
  camtel: "Camtel",
  nexttel: "Nexttel",
  unknown: "Unknown",
};

const numberFormat = new Intl.NumberFormat("en-US");

function formatCount(n: number): string {
  return numberFormat.format(n);
}

function formatPercent(ratio: number): string {
  return `${(ratio * 100).toFixed(ratio >= 0.1 ? 0 : 1)}%`;
}

function StatCard({
  title,
  value,
  caption,
  accent,
}: {
  title: string;
  value: string;
  caption: string;
  accent?: "uncertain" | undefined;
}) {
  return (
    <Card>
      <CardHeader title={title} />
      <CardBody>
        <p
          className={
            accent === "uncertain"
              ? "font-medium text-title tracking-tight text-state-uncertain-fg"
              : "font-medium text-title tracking-tight text-foreground"
          }
        >
          {value}
        </p>
        <p className="mt-1 text-caption text-muted-foreground">{caption}</p>
      </CardBody>
    </Card>
  );
}

export function DashboardScreen() {
  const summaryQuery = trpc.dashboard.summary.useQuery(undefined, {
    refetchInterval: REFETCH_INTERVAL_MS,
  });

  const data = summaryQuery.data;
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

  return (
    <main className="mx-auto flex max-w-[1400px] flex-col gap-6 px-6 py-10">
      <header className="flex items-start justify-between gap-4 border-edge border-b pb-6">
        <div>
          <p className="font-mono text-micro text-subtle-foreground tracking-[0.03em]">
            vsms admin console
          </p>
          <h1 className="mt-1 font-medium text-foreground text-title">Dashboard</h1>
          <p className="mt-1 max-w-xl text-body text-muted-foreground">
            Throughput, delivery, and backlog at a glance. Refreshes every{" "}
            {Math.round(REFETCH_INTERVAL_MS / 1000)}s.
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
            href="/messages"
            className="text-caption text-muted-foreground underline decoration-edge-strong underline-offset-2 hover:decoration-foreground"
          >
            Messages
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
            href="/providers"
            className="text-caption text-muted-foreground underline decoration-edge-strong underline-offset-2 hover:decoration-foreground"
          >
            Providers
          </a>
          <a
            href="/routes"
            className="text-caption text-muted-foreground underline decoration-edge-strong underline-offset-2 hover:decoration-foreground"
          >
            Routes
          </a>
          <a
            href="/simulator"
            className="text-caption text-muted-foreground underline decoration-edge-strong underline-offset-2 hover:decoration-foreground"
          >
            Simulator
          </a>
          {/* #52/#58: these five screens don't otherwise appear in any
              other screen's own nav block — see `console-nav.tsx`'s own
              doc for why they share a small component among themselves
              rather than each pre-existing screen's header being edited
              too. Added here so they're reachable by click from the
              console's own hub, not only by typing a URL. */}
          <a
            href="/apps"
            className="text-caption text-muted-foreground underline decoration-edge-strong underline-offset-2 hover:decoration-foreground"
          >
            Apps
          </a>
          <a
            href="/users"
            className="text-caption text-muted-foreground underline decoration-edge-strong underline-offset-2 hover:decoration-foreground"
          >
            Users
          </a>
          <a
            href="/opt-outs"
            className="text-caption text-muted-foreground underline decoration-edge-strong underline-offset-2 hover:decoration-foreground"
          >
            Opt-outs
          </a>
          <a
            href="/audit-log"
            className="text-caption text-muted-foreground underline decoration-edge-strong underline-offset-2 hover:decoration-foreground"
          >
            Audit log
          </a>
          <a
            href="/settings"
            className="text-caption text-muted-foreground underline decoration-edge-strong underline-offset-2 hover:decoration-foreground"
          >
            Settings
          </a>
          <ThemeToggle />
        </div>
      </header>

      <div className="rounded-sm border border-edge bg-surface-2 px-3 py-2 text-caption text-muted-foreground">
        {data?.appId === undefined ? (
          <>
            You're reading this as yourself — message- and webhook-based tiles below cover{" "}
            <span className="font-mono text-foreground">every app</span> in this deployment, not
            one.{" "}
          </>
        ) : (
          <>
            Message- and webhook-based tiles below are scoped to{" "}
            <span className="font-mono text-foreground">this app only</span> — the console's own
            service-account token can only read the one app it belongs to.{" "}
          </>
        )}
        <span className="font-mono text-foreground">Job backlog</span> is always system-wide,
        because <span className="font-mono text-foreground">Job</span> has no app boundary to scope
        by. Neither is a filter, and neither is a bug.
      </div>

      {summaryQuery.isError && (
        <div className="rounded-sm border border-state-danger-border bg-state-danger-bg px-3 py-2 text-caption text-state-danger-fg">
          Couldn't load the dashboard: {summaryQuery.error.message}
        </div>
      )}

      {summaryQuery.isLoading ? (
        <div className="grid grid-cols-2 gap-4 sm:grid-cols-4">
          {Array.from({ length: 4 }).map((_, i) => (
            // biome-ignore lint/suspicious/noArrayIndexKey: static skeleton tiles, never reordered
            <Card key={i}>
              <CardBody className="pt-4">
                <Skeleton className="h-8 w-20" />
                <Skeleton className="mt-2 h-3 w-32" />
              </CardBody>
            </Card>
          ))}
        </div>
      ) : (
        <div className="grid grid-cols-2 gap-4 sm:grid-cols-4">
          <StatCard
            title="Queue depth"
            value={formatCount(data?.queueDepth ?? 0)}
            caption="accepted + queued + routed, right now"
          />
          <StatCard
            title="Job backlog"
            value={formatCount(data?.jobBacklog ?? 0)}
            caption="pending + running, system-wide"
          />
          <StatCard
            title="Outbox depth"
            value={formatCount(data?.outboxDepth ?? 0)}
            caption="webhook attempts pending or in flight"
          />
          <StatCard
            title="Stuck messages"
            value={formatCount(data?.stuckMessages ?? 0)}
            caption="uncertain + undelivered — never confirmed either way"
            accent={(data?.stuckMessages ?? 0) > 0 ? "uncertain" : undefined}
          />
        </div>
      )}

      <div className="grid grid-cols-1 gap-4 lg:grid-cols-2">
        <Card>
          <CardHeader title="Throughput" meta="last 6 hours, oldest first" />
          <CardBody>
            {buckets.length > 0 ? (
              <>
                <HourlyBars
                  bars={buckets.map((bucket) => ({
                    value: bucket.totalCount,
                    label: `${new Date(bucket.bucketStart).toLocaleTimeString([], {
                      hour: "2-digit",
                    })}: ${formatCount(bucket.totalCount)} messages`,
                  }))}
                  colorClassName="bg-state-neutral-fg"
                />
                <p className="mt-3 text-body text-foreground">
                  {formatCount(currentHour?.totalCount ?? 0)}{" "}
                  <span className="text-caption text-muted-foreground">this hour</span>
                </p>
                <p className="text-caption text-muted-foreground">
                  {throughputDelta == null
                    ? "no prior hour to compare yet"
                    : throughputDelta === 0
                      ? "flat vs. the previous hour"
                      : `${throughputDelta > 0 ? "+" : ""}${formatCount(throughputDelta)} vs. the previous hour`}
                </p>
              </>
            ) : (
              <p className="text-caption text-muted-foreground">Loading…</p>
            )}
          </CardBody>
        </Card>

        <Card>
          <CardHeader
            title="UCS-2 ratio"
            meta="last 6 hours, oldest first"
            action={
              ucs2Jumped ? (
                <span className="rounded-sm border border-state-uncertain-border bg-state-uncertain-bg px-2 py-0.5 text-caption text-state-uncertain-fg">
                  jumped
                </span>
              ) : undefined
            }
          />
          <CardBody>
            {buckets.length > 0 ? (
              <>
                <HourlyBars
                  bars={buckets.map((bucket) => ({
                    value: bucket.totalCount > 0 ? bucket.ucs2Count / bucket.totalCount : null,
                    label:
                      bucket.totalCount > 0
                        ? `${new Date(bucket.bucketStart).toLocaleTimeString([], {
                            hour: "2-digit",
                          })}: ${formatPercent(bucket.ucs2Count / bucket.totalCount)} UCS-2 (${bucket.ucs2Count}/${bucket.totalCount})`
                        : `${new Date(bucket.bucketStart).toLocaleTimeString([], {
                            hour: "2-digit",
                          })}: no messages`,
                  }))}
                  colorClassName="bg-state-uncertain-fg"
                />
                <p className="mt-3 text-body text-foreground">
                  {currentRatio == null ? "—" : formatPercent(currentRatio)}{" "}
                  <span className="text-caption text-muted-foreground">this hour</span>
                </p>
                <p className="text-caption text-muted-foreground">
                  {previousRatio == null
                    ? "no prior hour to compare yet"
                    : `previous hour: ${formatPercent(previousRatio)}`}
                  {ucs2Jumped &&
                    " — a template change or a smart apostrophe/accent is a common cause"}
                </p>
              </>
            ) : (
              <p className="text-caption text-muted-foreground">Loading…</p>
            )}
          </CardBody>
        </Card>
      </div>

      <Card>
        <CardHeader
          title="Delivery rate by operator"
          meta="trailing 24 hours, terminal messages only"
        />
        <CardBody>
          {allOperatorsQuiet && (
            <p className="text-caption text-muted-foreground">
              No terminal messages in the last 24 hours for this app.
            </p>
          )}
          {operatorRows.length > 0 && (
            <div className="flex flex-col gap-3">
              {operatorRows.map((row) => {
                const ratio = row.delivered / row.terminalTotal;
                return (
                  <div key={row.operator} className="flex items-center gap-3">
                    <span className="w-20 shrink-0 text-caption text-foreground">
                      {OPERATOR_LABELS[row.operator]}
                    </span>
                    <div className="h-2 flex-1 overflow-hidden rounded-full bg-surface-3">
                      <div
                        className="h-full rounded-full bg-state-success-fg"
                        style={{ width: `${Math.round(ratio * 100)}%` }}
                      />
                    </div>
                    <span className="w-32 shrink-0 text-right font-mono text-caption text-muted-foreground">
                      {formatPercent(ratio)} ({formatCount(row.delivered)}/
                      {formatCount(row.terminalTotal)})
                    </span>
                  </div>
                );
              })}
            </div>
          )}
          <p className="mt-3 text-caption text-subtle-foreground">
            Excludes {formatCount(data?.stuckMessages ?? 0)} stuck message
            {(data?.stuckMessages ?? 0) === 1 ? "" : "s"} (uncertain/undelivered) — see the tile
            above. Counting those as failed would overstate failure; counting them as delivered
            would be a lie.
          </p>
        </CardBody>
      </Card>

      <Card>
        <CardHeader title="Provider balance" />
        <CardBody>
          <p className="text-caption text-muted-foreground">
            Not available. <span className="font-mono text-foreground">poll_balance</span> (§7.5)
            was never built — there is no source of truth for provider balance anywhere in this
            system yet. This card intentionally shows no number rather than a fabricated one.
          </p>
        </CardBody>
      </Card>

      <p className="text-caption text-subtle-foreground">
        Outbox age and poison-row alerting live in Prometheus, not here — see{" "}
        <span className="font-mono">deploy/prometheus/alerts.yml</span>. This screen's{" "}
        <span className="font-mono text-foreground">Outbox depth</span> tile is a genuine current
        count of <span className="font-mono text-foreground">WebhookAttempt</span> rows, a different
        table from the framework's own internal event outbox those alerts describe.
      </p>
    </main>
  );
}
