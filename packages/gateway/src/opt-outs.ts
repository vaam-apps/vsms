import "server-only";

// `GET /opt_outs`, `DELETE /opt_outs/{id}`, and the two procedures #58
// added specifically because a plain `POST /opt_outs` structurally cannot
// work from this console: `OptOut.msisdnHash` is a peppered HMAC
// (`SMS_HASH_PEPPER`, `crates/sms-api/src/pepper.rs`) this console has no
// access to, so recording or searching an opt-out by its real MSISDN needs
// a procedure that hashes server-side — `POST /$procs/recordOptOut` and
// `POST /$procs/searchOptOutByMsisdn`, see `schema.cstack`'s own comments
// on each for the full reasoning.
//
// # What a search "not found" can honestly mean
//
// [`searchOptOutByMsisdn`] can never distinguish "this number never opted
// out" from "it did, under a `SMS_HASH_PEPPER` value that has since
// rotated" — a rotation orphans every `msisdnHash` computed under the old
// pepper, silently, permanently (`OPEN_QUESTIONS.md` §3.1). This module
// does not hide that: [`SearchOptOutResult`] carries no field that could
// claim otherwise, and `opt-outs-screen.tsx`'s own copy states the caveat
// next to every "not found" result rather than only in a comment nobody
// reading the UI will ever see.

import { env } from "@vsms/env";
import { fetch as undiciFetch } from "undici";
import { gatewayAgent } from "./dispatcher";
import { mapGatewayError } from "./errors";
import { parseGatewayJson } from "./json";
import { invalidateUpstreamAccessToken, resolveUpstreamAccessToken } from "./request-credential";

/** `schema.cstack`'s `OptOutSource`, verbatim. */
export type OptOutSource = "inbound_stop" | "admin" | "import" | "operator";

export interface OptOutRecord {
  id: string;
  msisdnHash: string;
  /** `@pii` — see AGENTS.md §2.0: no field-level read masking exists in
   * this framework, so the API genuinely returns this in the clear to
   * every role `OptOut.read` admits. Rendered through `@vsms/ui`'s own
   * `MsisdnDisplay` (masked by default) rather than printed bare, the same
   * discipline `messages-screen.tsx` already applies to `Message.msisdn`. */
  msisdn: string;
  source: OptOutSource;
  scope: string;
  reason?: string | undefined;
  optedOutAt: string;
  createdAt: string;
}

const LIST_FIELDS = [
  "id",
  "msisdnHash",
  "msisdn",
  "source",
  "scope",
  "reason",
  "optedOutAt",
  "createdAt",
] as const;

export type OptOutListItem = Pick<OptOutRecord, (typeof LIST_FIELDS)[number]>;

type UndiciResponse = Awaited<ReturnType<typeof undiciFetch>>;

function gatewayUrl(path: string, query: Record<string, string | number | undefined>): string {
  const url = new URL(path, env.SMS_API_URL);
  for (const [key, value] of Object.entries(query)) {
    if (value !== undefined) url.searchParams.set(key, String(value));
  }
  return url.toString();
}

async function authedRequest(
  url: string,
  init: { method: "GET" | "POST" | "DELETE"; body?: string },
): Promise<UndiciResponse> {
  const attempt = async (): Promise<UndiciResponse> => {
    const token = await resolveUpstreamAccessToken();
    return undiciFetch(url, {
      method: init.method,
      headers: {
        accept: "application/json",
        ...(init.body !== undefined ? { "content-type": "application/json" } : {}),
        authorization: `Bearer ${token}`,
      },
      ...(init.body !== undefined ? { body: init.body } : {}),
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

interface OptOutsPage {
  items: OptOutRecord[];
  totalCount: number;
  pageInfo: {
    limit: number;
    offset: number | null;
    hasNextPage: boolean;
    hasPreviousPage: boolean;
  };
}

function pickListFields(record: OptOutRecord): OptOutListItem {
  const out = {} as OptOutListItem;
  for (const field of LIST_FIELDS) {
    // biome-ignore lint/suspicious/noExplicitAny: narrowing a mapped-tuple key back to its own field
    (out as any)[field] = record[field];
  }
  return out;
}

/** `GET /opt_outs`, newest first, one bounded page — the recent-activity
 * table `opt-outs-screen.tsx` shows alongside the search box. */
export async function listOptOuts(limit = 100): Promise<OptOutListItem[]> {
  const url = gatewayUrl("/opt_outs", {
    limit,
    sort: "-optedOutAt",
    fields: LIST_FIELDS.join(","),
  });
  const response = await authedRequest(url, { method: "GET" });
  const parsed = await parseGatewayJson(response);
  if (!response.ok) {
    throw mapGatewayError(response.status, parsed, "listOptOuts");
  }
  const page = parsed as OptOutsPage;
  return page.items.map(pickListFields);
}

export interface OptOutSummary {
  id: string;
  msisdnHash: string;
  source: OptOutSource;
  scope: string;
  reason?: string | undefined;
  optedOutAt: string;
  createdAt: string;
}

export interface SearchOptOutResult {
  optOut?: OptOutSummary | undefined;
}

/** `POST /$procs/searchOptOutByMsisdn` — see module doc for what a "not
 * found" result can and cannot mean. The wire sends `{"optOut": null}` for
 * "not found," not an omitted key — `./json.ts`'s shared seam converts that
 * to `optOut: undefined` before this function ever sees the body, so the
 * cast below is exactly [`SearchOptOutResult`]'s own shape, not a
 * `| null` variant of it. */
export async function searchOptOutByMsisdn(msisdn: string): Promise<SearchOptOutResult> {
  const url = new URL("/$procs/searchOptOutByMsisdn", env.SMS_API_URL).toString();
  const response = await authedRequest(url, {
    method: "POST",
    body: JSON.stringify({ args: { msisdn } }),
  });
  const parsed = await parseGatewayJson(response);
  if (!response.ok) {
    throw mapGatewayError(response.status, parsed, "searchOptOutByMsisdn");
  }
  return parsed as SearchOptOutResult;
}

export interface RecordOptOutFields {
  msisdn: string;
  source: OptOutSource;
  scope: string;
  reason?: string | undefined;
}

/** `POST /$procs/recordOptOut` — see module doc. Runs under `sys()`
 * server-side regardless of the caller's own role, which is what lets a
 * `support`-role caller succeed here even though `OptOut.create`'s own
 * `@@allow` never admitted `hasRole('support')` directly (#58's own
 * finding — `crates/sms-api/src/procedures.rs::create_opt_out_entry`'s own
 * doc has the full reasoning). */
export async function recordOptOut(fields: RecordOptOutFields): Promise<OptOutRecord> {
  const url = new URL("/$procs/recordOptOut", env.SMS_API_URL).toString();
  const response = await authedRequest(url, {
    method: "POST",
    body: JSON.stringify({ args: fields }),
  });
  const parsed = await parseGatewayJson(response);
  if (!response.ok) {
    throw mapGatewayError(response.status, parsed, "recordOptOut");
  }
  return parsed as OptOutRecord;
}

/** `DELETE /opt_outs/{id}` — `OptOut.delete`'s own `@@allow` is
 * `hasRole('owner') || hasRole('admin')`, narrower than `create`'s — a
 * `support`/`operator` caller can record an opt-out through the procedure
 * above but cannot remove one; that is this schema's own existing policy,
 * unchanged by #58. */
export async function deleteOptOut(id: string): Promise<void> {
  const url = gatewayUrl(`/opt_outs/${encodeURIComponent(id)}`, {});
  const response = await authedRequest(url, { method: "DELETE" });
  if (!response.ok) {
    const parsed = await parseGatewayJson(response);
    throw mapGatewayError(response.status, parsed, "deleteOptOut");
  }
}
