import "server-only";

// `POST /$procs/dashboardSummary` — the data layer behind #49's Dashboard
// screen. Same temporary hand-written seam as `client.ts`/`workers.ts` (see
// `client.ts`'s module doc for why).
//
// Types transcribed from `schema.cstack`'s `DashboardSummary`/
// `HourlyBucket`/`OperatorDeliveryStats` — see
// `crates/sms-api/src/procedures.rs`'s own doc on `dashboard_snapshot` for
// exactly which model each field reads, under which context, and why a
// bare percentage can't stand in for `hourlyBuckets`' own trend.

import { env } from "@vsms/env";
import { fetch as undiciFetch } from "undici";
import type { OperatorCode } from "./client";
import { gatewayAgent } from "./dispatcher";
import { mapGatewayError } from "./errors";
import { getAccessToken, invalidateAccessToken } from "./token";

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
   * system context). Today, the console's own machine credential is
   * always app-scoped (#211) — this is always present in practice, but
   * the screen still renders off this field rather than assuming it,
   * matching `messages-screen.tsx`'s own precedent of stating a scope
   * limit rather than leaving it to be inferred. */
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

/** The wire's `appId` is an explicit JSON `null` when absent, not an
 * omitted key (`Cuid?` in `schema.cstack`) — the same trap
 * `routes.ts`/`providers.ts`'s own `normalizeRoute`/`normalizeProvider`
 * already document and guard against (found live wiring #54: `!==
 * undefined` is `true` for a JSON `null`). One normalization point here
 * too, so nothing downstream needs its own `!= null` check. */
function normalizeDashboardSummary(raw: DashboardSummary): DashboardSummary {
  return { ...raw, appId: raw.appId ?? undefined };
}

type UndiciResponse = Awaited<ReturnType<typeof undiciFetch>>;

function procedureUrl(procedure: string): string {
  return new URL(`/$procs/${procedure}`, env.SMS_API_URL).toString();
}

async function parseJsonBody(response: UndiciResponse): Promise<unknown> {
  const text = await response.text();
  if (text.length === 0) return undefined;
  try {
    return JSON.parse(text);
  } catch {
    return { code: "UNPARSEABLE_RESPONSE", message: text };
  }
}

/**
 * `POST /$procs/dashboardSummary` — a fresh (or up-to-15s-old, per the
 * server's own `dashboard_cache`) snapshot for the Dashboard screen. No
 * caller-supplied args; `{}` as the body, matching every other `$procs`
 * call's envelope (`{ args }`).
 */
export async function dashboardSummary(): Promise<DashboardSummary> {
  const url = procedureUrl("dashboardSummary");

  const attempt = async (): Promise<UndiciResponse> => {
    const token = await getAccessToken();
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
    invalidateAccessToken();
    response = await attempt();
  }

  const parsed = await parseJsonBody(response);
  if (!response.ok) {
    throw mapGatewayError(response.status, parsed, "dashboardSummary");
  }
  return normalizeDashboardSummary(parsed as DashboardSummary);
}
