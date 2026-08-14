import "server-only";

// `POST /$procs/simulateRoute` — #54's route simulator: "given this
// recipient, class and app, which route wins and why," without sending
// anything. Same temporary hand-written seam as `workers.ts`'s own
// `workerLocks` call (see `client.ts`'s module doc for why).
//
// Every type below is a rendering of `sms_routing::Decision`
// (`backends/crates/sms-routing`), transcribed from `schema.cstack`'s
// `SimulateRouteResult`/`RouteEvaluationInfo`/`TieBreakInfo`/
// `RouteWinnerInfo` — see `backends/crates/sms-api/src/route_simulator.rs`'s own
// module doc for the guarantee that this is a faithful rendering of the
// engine's own answer, never a second implementation of matching.
//
// A `null` -> `undefined` normalization *is* applied, not zero, the same
// fix `routes.ts`'s own module doc records at length for `Route`'s `match*`
// columns. Found the same way, live, against a real `just demo` — a
// single-route candidate with nothing to tie-break sent `"tieBreak": null`,
// and `simulator-screen.tsx`'s own `result.tieBreak !== undefined` check
// let that null through, crashing the render with "Cannot read properties
// of null (reading 'priority')" the instant it tried `result.tieBreak.
// priority`.
//
// **#221 correction:** this module used to carry its own `normalizeResult`,
// hand-enumerating `tieBreak`, `winner`, `winner.failoverRouteId`, and
// every optional field on each `RouteEvaluationInfo`. `SimulateRouteResult`
// is ordinary structured JSON all the way down — nested objects and arrays
// of objects, no JSON-encoded-as-`String` field anywhere in it — so
// `frontends/packages/gateway/src/json.ts`'s generic recursive walk covers every one
// of those fields, at every depth, without this module needing to name a
// single one of them. See that file's own module doc for the full
// reasoning and for why it's safe to apply without enumerating fields here.

import { env } from "@vsms/env";
import { fetch as undiciFetch } from "undici";
import { gatewayAgent } from "./dispatcher";
import { mapGatewayError } from "./errors";
import { parseGatewayJson } from "./json";
import { invalidateUpstreamAccessToken, resolveUpstreamAccessToken } from "./request-credential";

export type SimulateOperatorCode = "mtn" | "orange" | "camtel" | "nexttel" | "unknown";
export type SimulateMessageClass = "otp" | "transactional" | "notification" | "marketing";

export type RouteOutcomeKind =
  | "excluded"
  | "disabled"
  | "predicate_failed"
  | "provider_unavailable"
  | "eligible";

export type PredicateKind = "operator" | "class" | "app_id" | "prefix";

export interface RouteEvaluationInfo {
  routeId: string;
  routeName: string;
  priority: number;
  weight: number;
  providerId: string;
  outcome: RouteOutcomeKind;
  winningBand: boolean;
  predicateKind?: PredicateKind | undefined;
  predicateExpected?: string | undefined;
  predicateActual?: string | undefined;
  unavailableReason?: string | undefined;
}

export interface TieBreakRangeInfo {
  routeId: string;
  weight: number;
  low: number;
  high: number;
}

export interface TieBreakInfo {
  priority: number;
  draw: number;
  ranges: TieBreakRangeInfo[];
  winnerRouteId: string;
}

export interface RouteWinnerInfo {
  routeId: string;
  providerId: string;
  failoverRouteId?: string | undefined;
}

export interface SimulateRouteInput {
  msisdn: string;
  class: SimulateMessageClass;
  appId: string;
  /** Omit for a fresh, realistic random draw; supply to replay an exact
   * tie-break — see `backends/crates/sms-routing/src/engine.rs`'s own doc on why
   * the draw is injected rather than generated internally. */
  draw?: number | undefined;
}

export interface SimulateRouteResult {
  operator: SimulateOperatorCode;
  msisdnNational: string;
  noRoutesConfigured: boolean;
  evaluations: RouteEvaluationInfo[];
  tieBreak?: TieBreakInfo | undefined;
  winner?: RouteWinnerInfo | undefined;
}

type UndiciResponse = Awaited<ReturnType<typeof undiciFetch>>;

function procedureUrl(procedure: string): string {
  return new URL(`/$procs/${procedure}`, env.SMS_API_URL).toString();
}

/**
 * `POST /$procs/simulateRoute`. Reads `Route`/`Provider` under the
 * procedure's own `sys()` context server-side (`route_simulator.rs`'s own
 * doc) — this console's `route:read` scope is what Layer 2's
 * `require_permission` checks, not anything this function sends.
 */
export async function simulateRoute(input: SimulateRouteInput): Promise<SimulateRouteResult> {
  const url = procedureUrl("simulateRoute");
  const body = JSON.stringify({ args: input });

  const attempt = async (): Promise<UndiciResponse> => {
    const token = await resolveUpstreamAccessToken();
    return undiciFetch(url, {
      method: "POST",
      headers: {
        "content-type": "application/json",
        accept: "application/json",
        authorization: `Bearer ${token}`,
      },
      body,
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
    throw mapGatewayError(response.status, parsed, "simulateRoute");
  }
  return parsed as SimulateRouteResult;
}
