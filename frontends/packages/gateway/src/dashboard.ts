import "server-only";

// `POST /$procs/dashboardSummary` — the data layer behind #49's Dashboard
// screen. Same temporary hand-written seam as `client.ts`/`workers.ts` (see
// `client.ts`'s module doc for why).
//
// Types transcribed from `schema.cstack`'s `DashboardSummary`/
// `HourlyBucket`/`OperatorDeliveryStats` — see
// `backends/crates/sms-api/src/procedures.rs`'s own doc on `dashboard_snapshot` for
// exactly which model each field reads, under which context, and why a
// bare percentage can't stand in for `hourlyBuckets`' own trend.

import { env } from "@vsms/env";
import { fetch as undiciFetch } from "undici";
import type { OperatorCode } from "./client";
import { gatewayAgent } from "./dispatcher";
import { mapGatewayError } from "./errors";
import { parseGatewayJson } from "./json";
import { invalidateUpstreamAccessToken, resolveUpstreamAccessToken } from "./request-credential";

/** One rolling hour of `Message.createdAt`, oldest first. `totalCount` is
 * that hour's throughput; `ucs2Count` is how many of those encoded UCS-2 —
 * both read off the same underlying scan server-side, so the throughput
 * and UCS-2-ratio tiles never disagree after two independent
 * computations. */
export interface HourlyBucket {
  bucketStart: string;
  totalCount: number;
  ucs2Count: number;
}

/** Delivery rate by operator, over a trailing 24h window. `terminalTotal`
 * deliberately excludes `uncertain`/`undelivered` — see `DashboardSummary`'s
 * own `stuckMessages` doc for why folding either in would misstate the
 * ratio in one direction or the other. */
export interface OperatorDeliveryStats {
  operator: OperatorCode;
  delivered: number;
  terminalTotal: number;
}

export interface DashboardSummary {
  generatedAt: string;
  /** Absent when the caller isn't scoped to one app (a real human or
   * system context). Before #211, this call always ran as the console's
   * own machine credential, so `appId` was always present in practice.
   * #211 forwards the signed-in human's own session token instead — and
   * `Message`/`WebhookAttempt`'s own `@@allow` (`schema.cstack`) admits
   * `auth().kind == "user"` unconditionally, unscoped by `appId` — so for
   * any signed-in human, regardless of role, this is now genuinely
   * `undefined` in practice: the numbers below cover every app, not one.
   * See `dashboard-screen.tsx`'s own "Why the scope banner" section. */
  appId?: string | undefined;
  /** Current count of `Message` rows in `{accepted, queued, routed}` — a
   * live gauge, not a rate. */
  queueDepth: number;
  /** Current count of `Job` rows in `{pending, running}`, system-wide —
   * `Job` has no `appId` to scope by (the Jobs screen's own precedent). */
  jobBacklog: number;
  /** Current count of `WebhookAttempt` rows in `{pending, delivering}`,
   * scoped to this app's own endpoints. A genuine depth count — distinct
   * from, and does not recompute, the Prometheus
   * `sms_webhook_outbox_oldest_undelivered_age_seconds` /
   * `sms_event_outbox_poison_rows` gauges, which describe a different
   * table entirely (the framework's own internal event outbox, not this
   * schema's `WebhookAttempt` rows). See the Dashboard screen's own
   * caption for why both are shown rather than one standing in for the
   * other. */
  outboxDepth: number;
  /** Current count of `Message` rows in `{uncertain, undelivered}` —
   * never confirmed either delivered or failed. Reported on its own,
   * never folded into `operatorStats`. */
  stuckMessages: number;
  /** Trailing 24h, one row per `OperatorCode` variant, including
   * zero-count ones. */
  operatorStats: OperatorDeliveryStats[];
  /** Six rolling hours, oldest first. `hourlyBuckets[5]` is the current,
   * in-progress hour. */
  hourlyBuckets: HourlyBucket[];
}

type UndiciResponse = Awaited<ReturnType<typeof undiciFetch>>;

function procedureUrl(procedure: string): string {
  return new URL(`/$procs/${procedure}`, env.SMS_API_URL).toString();
}

/**
 * `POST /$procs/dashboardSummary` — a fresh (or up-to-15s-old, per the
 * server's own `dashboard_cache`) snapshot for the Dashboard screen. No
 * caller-supplied args; `{}` as the body, matching every other `$procs`
 * call's envelope (`{ args }`).
 *
 * The wire's `appId` is an explicit JSON `null` when absent, not an omitted
 * key — `./json.ts`'s shared seam converts that to `undefined` for this and
 * every other response this package parses (#221), so this function no
 * longer needs its own `normalizeDashboardSummary`.
 */
export async function dashboardSummary(): Promise<DashboardSummary> {
  const url = procedureUrl("dashboardSummary");

  const attempt = async (): Promise<UndiciResponse> => {
    const token = await resolveUpstreamAccessToken();
    return undiciFetch(url, {
      method: "POST",
      headers: {
        "content-type": "application/json",
        accept: "application/json",
        authorization: `Bearer ${token}`,
      },
      body: JSON.stringify({ args: {} }),
      dispatcher: gatewayAgent(),
    });
  };

  let response = await attempt();
  if (response.status === 401) {
    invalidateUpstreamAccessToken();
    response = await attempt();
  }

  const parsed = await parseGatewayJson(response);
  if (!response.ok) {
    throw mapGatewayError(response.status, parsed, "dashboardSummary");
  }
  return parsed as DashboardSummary;
}
