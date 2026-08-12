import "server-only";

// `GET /jobs` and `POST /$procs/requeueJob` — the data layer behind #56's
// Jobs screen. Same temporary hand-written seam as `client.ts`/`messages.ts`
// (see `client.ts`'s module doc for why: T3/`packages/sms-client` is
// blocked on an upstream cratestack release).
//
// # Reusing `messages.ts`'s own live findings rather than re-probing
//
// `Job` shares `GET /messages`'s REST list mechanics exactly — both are
// `@@paged` models served through the same generated router, `Accept:
// application/json`, and the same query grammar (`ci/assert-no-raw-sqlx.sh`
// aside, this is generated code, not model-specific behaviour). Three
// findings `messages.ts`'s own module doc verified live are inherited here
// without a second live probe, since they're properties of the grammar, not
// of `Message` specifically:
//
// 1. The envelope is `{ items, totalCount, pageInfo }`, `fields=` narrows
//    the projection, `limit` is capped at 1000 server-side.
// 2. **Enum columns are never filterable via bare query params or `where=`**
//    (`messages.ts`, finding 6) — confirmed for `Message.state`, and this
//    module assumes the identical grammar applies to `Job.state` (also an
//    enum column, same emitter, same router) rather than re-verifying it
//    live a second time for a different model. If that assumption is ever
//    wrong, `listJobs` below still works — it filters `state` here in Node
//    either way — it would just mean the server *could* have done it and
//    didn't need to; the assumption fails safe, not open.
// 3. Row-level policy scoping (`appId == auth().appId`) doesn't apply here:
//    `Job` carries no `appId` at all (`schema.cstack`'s own comment on why
//    its `@@allow` admits `auth().kind == "app"` unscoped) — the console's
//    token sees the whole system's job backlog, not just "its own" jobs,
//    because there is no "its own" for a model with no tenant column.
//    `jobs-screen.tsx` states this on screen rather than leaving an
//    unfamiliar-looking global list to be mistaken for a bug.
//
// `state` filtering happens here, in Node, exactly like `listMessages` —
// never pretending the server did it.

import { env } from "@vsms/env";
import { fetch as undiciFetch } from "undici";
import { gatewayAgent } from "./dispatcher";
import { mapGatewayError } from "./errors";
import { invalidateUpstreamAccessToken, resolveUpstreamAccessToken } from "./request-credential";

/** `job_state_transitions` (`schema/migrations/postgres/0002_bootstrap/
 * up.sql`), verbatim — mirrors `@vsms/ui`'s own `JobState` (`status-
 * tokens.ts`), duplicated rather than imported because `@vsms/gateway` is
 * server-only and has no reason to depend on the UI package. */
export type JobState = "pending" | "running" | "succeeded" | "failed" | "dead" | "cancelled";

const MAX_SERVER_LIMIT = 1000;

/** The full row shape `GET /jobs`/`GET /jobs/{id}` can return — transcribed
 * from `schema.cstack`'s `Job` model, `payload` excluded from the list
 * projection (same reasoning `messages.ts` excludes `body`: potentially
 * large, and this screen's own table has no use for the raw JSON). */
export interface JobRecord {
  id: string;
  kind: string;
  dedupeKey?: string | undefined;
  state: JobState;
  priority: number;
  runAt: string;
  leaseOwner?: string | undefined;
  leaseUntil?: string | undefined;
  attempts: number;
  maxAttempts: number;
  lastError?: string | undefined;
  startedAt?: string | undefined;
  finishedAt?: string | undefined;
  version: number;
  createdAt: string;
  updatedAt: string;
}

const LIST_FIELDS = [
  "id",
  "kind",
  "dedupeKey",
  "state",
  "priority",
  "runAt",
  "leaseOwner",
  "leaseUntil",
  "attempts",
  "maxAttempts",
  "lastError",
  "startedAt",
  "finishedAt",
  "version",
  "createdAt",
  "updatedAt",
] as const;

export type JobListItem = Pick<JobRecord, (typeof LIST_FIELDS)[number]>;

export interface ListJobsInput {
  state?: JobState | undefined;
  /** Substring match against `kind`, applied here in Node — `kind` is a
   * free-form `String` column (`schema.cstack`'s own comment: "not a
   * schema enum — §7.5's table is documentation, not a closed set the type
   * system enforces"), so there is no fixed list to render as a `<Select>`
   * the way `state` gets one. */
  kind?: string | undefined;
  limit?: number | undefined;
  offset?: number | undefined;
}

export interface ListJobsResult {
  items: JobListItem[];
  totalCount: number;
  hasNextPage: boolean;
  /** True when the fetched window (capped at `MAX_SERVER_LIMIT`) may not
   * contain every job matching the filter — same honesty contract
   * `listMessages`'s own `truncated` field documents. */
  truncated: boolean;
}

const DEFAULT_LIST_LIMIT = 100;

type UndiciResponse = Awaited<ReturnType<typeof undiciFetch>>;

function gatewayUrl(path: string, query: Record<string, string | number | undefined>): string {
  const url = new URL(path, env.SMS_API_URL);
  for (const [key, value] of Object.entries(query)) {
    if (value !== undefined) url.searchParams.set(key, String(value));
  }
  return url.toString();
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

/** `GET`/`POST` with a Bearer token, retrying once on an unexpected 401 —
 * the same shape `client.ts`'s `callProcedure` and `messages.ts`'s
 * `getJson` each already carry, duplicated a third time rather than
 * shared: all three are independently-replaceable halves of the same
 * temporary seam (T3), and none is large enough that sharing saves more
 * than it couples their futures together. */
async function authedRequest(
  url: string,
  init: { method: "GET" | "POST"; body?: string },
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

interface JobsPage {
  items: JobRecord[];
  totalCount: number;
  pageInfo: {
    limit: number;
    offset: number | null;
    hasNextPage: boolean;
    hasPreviousPage: boolean;
  };
}

function pickListFields(record: JobRecord): JobListItem {
  const out = {} as JobListItem;
  for (const field of LIST_FIELDS) {
    // biome-ignore lint/suspicious/noExplicitAny: narrowing a mapped-tuple key back to its own field
    (out as any)[field] = record[field];
  }
  return out;
}

/**
 * `GET /jobs`, windowed and filtered per this module's own doc. Scoped to
 * the *whole system's* backlog, not one app — see this module's doc,
 * point 3, for why.
 */
export async function listJobs(input: ListJobsInput = {}): Promise<ListJobsResult> {
  const limit = input.limit ?? DEFAULT_LIST_LIMIT;
  const offset = input.offset ?? 0;

  const url = gatewayUrl("/jobs", {
    limit: MAX_SERVER_LIMIT,
    sort: "-updatedAt",
    fields: LIST_FIELDS.join(","),
  });
  const response = await authedRequest(url, { method: "GET" });
  const parsed = await parseJsonBody(response);
  if (!response.ok) {
    throw mapGatewayError(response.status, parsed, "listJobs");
  }
  const page = parsed as JobsPage;
  const window = page.items;

  const filtered = window.filter((record) => {
    if (input.state !== undefined && record.state !== input.state) return false;
    if (input.kind !== undefined && !record.kind.includes(input.kind)) return false;
    return true;
  });

  const pageSlice = filtered.slice(offset, offset + limit);

  return {
    items: pageSlice.map(pickListFields),
    totalCount: filtered.length,
    hasNextPage: offset + limit < filtered.length,
    truncated: window.length >= MAX_SERVER_LIMIT,
  };
}

export interface RequeueJobResult {
  id: string;
  state: JobState;
  attempts: number;
  version: number;
  runAt: string;
}

/**
 * `POST /$procs/requeueJob` (#56) — resets a `dead` job to `pending` with a
 * fresh attempts counter. Anything but `dead` comes back as a `409`,
 * surfaced through `mapGatewayError` as a tRPC `CONFLICT` exactly like any
 * other gateway error — `jobs-screen.tsx` only ever renders the requeue
 * action for a row whose `state` is already `dead`, so a real conflict here
 * means the row moved between this screen's last poll and the click, not a
 * UI bug.
 */
export async function requeueJob(jobId: string): Promise<RequeueJobResult> {
  const url = gatewayUrl("/$procs/requeueJob", {});
  const response = await authedRequest(url, {
    method: "POST",
    body: JSON.stringify({ args: { jobId } }),
  });
  const parsed = await parseJsonBody(response);
  if (!response.ok) {
    throw mapGatewayError(response.status, parsed, "requeueJob");
  }
  return parsed as RequeueJobResult;
}
