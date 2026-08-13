import "server-only";

// `POST /$procs/auditLog` and `POST /$procs/auditChainStatus` — #58's
// read-only Audit log screen. Both are procedures, not a generated CRUD
// route: `cratestack_audit` is the framework's own internal bookkeeping
// table, not one of `schema.cstack`'s models, so there is no `GET
// /audit_log` route to call in the first place — see
// `crates/sms-api/src/audit_log.rs`'s own module doc for the full R1
// exception reasoning and for why this is genuinely read-only (checked
// live against a real Postgres, not assumed: no role, human or synthetic,
// can write an `AuditAnchor` row through any path this codebase exposes).

import { env } from "@vsms/env";
import { fetch as undiciFetch } from "undici";
import { gatewayAgent } from "./dispatcher";
import { mapGatewayError } from "./errors";
import { parseGatewayJson } from "./json";
import { invalidateUpstreamAccessToken, resolveUpstreamAccessToken } from "./request-credential";

export interface AuditLogEntry {
  eventId: string;
  model: string;
  operation: string;
  /** JSON-encoded text, not parsed further — the same convention
   * `Provider.config`/`Route.config` already use for a JSON-shaped
   * `String` column. Rendered through `@vsms/ui`'s `PayloadInspector`. */
  primaryKey: string;
  actor: string;
  tenant?: string | undefined;
  before?: string | undefined;
  after?: string | undefined;
  requestId?: string | undefined;
  occurredAt: string;
}

export interface AuditLogQuery {
  model?: string | undefined;
  operation?: string | undefined;
  actorId?: string | undefined;
  since?: string | undefined;
  until?: string | undefined;
  limit?: number | undefined;
  offset?: number | undefined;
}

export interface AuditLogPage {
  entries: AuditLogEntry[];
  hasMore: boolean;
}

export interface AuditChainStatus {
  latestAnchorId?: string | undefined;
  latestPeriodEnd?: string | undefined;
  latestRowCount?: number | undefined;
  linkageBreaks: string[];
  latestContentVerified?: boolean | undefined;
}

type UndiciResponse = Awaited<ReturnType<typeof undiciFetch>>;

async function authedPost(url: string, body: unknown): Promise<UndiciResponse> {
  const attempt = async (): Promise<UndiciResponse> => {
    const token = await resolveUpstreamAccessToken();
    return undiciFetch(url, {
      method: "POST",
      headers: {
        "content-type": "application/json",
        accept: "application/json",
        authorization: `Bearer ${token}`,
      },
      body: JSON.stringify(body),
      dispatcher: gatewayAgent(),
    });
  };
  let response = await attempt();
  if (response.status === 401) {
    invalidateUpstreamAccessToken();
    response = await attempt();
  }
  return response;
}

/** `POST /$procs/auditLog`. `AuditLogEntry`'s nullable fields (`tenant`,
 * `before`, `after`, `requestId`) no longer need a local `normalizeEntry` —
 * #221's shared seam (`./json.ts`) already converts the wire's `null` to
 * `undefined` for every response this module parses, including these,
 * *before* this function ever sees the body. `before`/`after`/`actor`/
 * `primaryKey` are themselves JSON-encoded `String` columns (see
 * `AuditLogEntry`'s own doc), so that seam's recursion never descends into
 * their own encoded contents either — see `./json.ts`'s module doc for the
 * full reasoning. */
export async function fetchAuditLog(query: AuditLogQuery): Promise<AuditLogPage> {
  const url = new URL("/$procs/auditLog", env.SMS_API_URL).toString();
  const response = await authedPost(url, { args: query });
  const parsed = await parseGatewayJson(response);
  if (!response.ok) {
    throw mapGatewayError(response.status, parsed, "auditLog");
  }
  return parsed as AuditLogPage;
}

/** `POST /$procs/auditChainStatus`. Same seam, same reasoning as
 * [`fetchAuditLog`] above. */
export async function fetchAuditChainStatus(): Promise<AuditChainStatus> {
  const url = new URL("/$procs/auditChainStatus", env.SMS_API_URL).toString();
  const response = await authedPost(url, {});
  const parsed = await parseGatewayJson(response);
  if (!response.ok) {
    throw mapGatewayError(response.status, parsed, "auditChainStatus");
  }
  return parsed as AuditChainStatus;
}
