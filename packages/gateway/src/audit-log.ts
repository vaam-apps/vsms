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

async function parseJsonBody(response: UndiciResponse): Promise<unknown> {
  const text = await response.text();
  if (text.length === 0) return undefined;
  try {
    return JSON.parse(text);
  } catch {
    return { code: "UNPARSEABLE_RESPONSE", message: text };
  }
}

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

function normalizeEntry(entry: AuditLogEntry): AuditLogEntry {
  return {
    ...entry,
    tenant: entry.tenant ?? undefined,
    before: entry.before ?? undefined,
    after: entry.after ?? undefined,
    requestId: entry.requestId ?? undefined,
  };
}

/** `POST /$procs/auditLog`. */
export async function fetchAuditLog(query: AuditLogQuery): Promise<AuditLogPage> {
  const url = new URL("/$procs/auditLog", env.SMS_API_URL).toString();
  const response = await authedPost(url, { args: query });
  const parsed = await parseJsonBody(response);
  if (!response.ok) {
    throw mapGatewayError(response.status, parsed, "auditLog");
  }
  const page = parsed as AuditLogPage;
  return { entries: page.entries.map(normalizeEntry), hasMore: page.hasMore };
}

/** `POST /$procs/auditChainStatus`. */
export async function fetchAuditChainStatus(): Promise<AuditChainStatus> {
  const url = new URL("/$procs/auditChainStatus", env.SMS_API_URL).toString();
  const response = await authedPost(url, {});
  const parsed = await parseJsonBody(response);
  if (!response.ok) {
    throw mapGatewayError(response.status, parsed, "auditChainStatus");
  }
  const status = parsed as AuditChainStatus;
  return {
    ...status,
    latestAnchorId: status.latestAnchorId ?? undefined,
    latestPeriodEnd: status.latestPeriodEnd ?? undefined,
    latestRowCount: status.latestRowCount ?? undefined,
    latestContentVerified: status.latestContentVerified ?? undefined,
  };
}
