import "server-only";

// `POST /$procs/workerLocks` — the data layer behind #57's Workers screen.
// Same temporary hand-written seam as `client.ts`/`jobs.ts` (see
// `client.ts`'s module doc for why).
//
// Types transcribed from `schema.cstack`'s `WorkerLockInfo`/
// `WorkerLocksResult` — see `backends/crates/sms-api/src/worker_locks.rs`'s own
// module doc for what `held`/`workerId`/`pid`/`heldSince` actually mean
// (verified live against a real Postgres, not assumed): at most one
// `held: true` row can ever exist per `role` — Postgres's own two-key
// advisory-lock exclusivity guarantees it — so this client never has to
// defend against "two holders" as a data shape; the interesting failure
// mode is a role showing `held: false` that operationally should be held.

import { env } from "@vsms/env";
import { fetch as undiciFetch } from "undici";
import { gatewayAgent } from "./dispatcher";
import { mapGatewayError } from "./errors";
import { parseGatewayJson } from "./json";
import { invalidateUpstreamAccessToken, resolveUpstreamAccessToken } from "./request-credential";

/** The six §7.1 role names, verbatim (`sms_worker::Role::as_str`). */
export type WorkerRole = "dispatch" | "drain" | "scheduler" | "hooks" | "jobs" | "smpp";

/**
 * **This module never had its own `normalize*` function either — another
 * latent instance of the bug #221 was filed over, alongside `jobs.ts`.**
 * `workerId`/`pid`/`heldSince` are `null` on the wire for a role currently
 * showing `held: false`, exactly the same shape as every other nullable
 * column this package has found the hard way; nothing had driven the
 * Workers screen through a standby role to render one before now.
 * `frontends/packages/gateway/src/json.ts`'s shared seam covers this module the same
 * as every other.
 */
export interface WorkerLockInfo {
  role: WorkerRole;
  /** `false` for `hooks`/`jobs` (`Cardinality::ScaleToN`) — those two never
   * take this lock at all, in any deployment, so `held` is always `false`
   * for them and that's not a fault. */
  singleton: boolean;
  held: boolean;
  /** Set from the holding worker's own `--worker-id`/`SMS_WORKER_ID`
   * (default `hostname:pid`) — the node identity `pg_locks.
   * application_name` carries because `RoleLease::try_acquire` (#57) sets
   * it explicitly on its dedicated connection. */
  workerId?: string | undefined;
  /** The Postgres backend pid holding the lock. Operator-facing detail —
   * useful when cross-referencing against `pg_stat_activity` by hand, not
   * otherwise actionable from this screen. */
  pid?: number | undefined;
  /** When the holding connection's own session started — to the second,
   * this *is* when the lease was acquired, since `RoleLease` opens a
   * connection dedicated to nothing but holding this one lock. */
  heldSince?: string | undefined;
}

export interface WorkerLocksResult {
  locks: WorkerLockInfo[];
}

type UndiciResponse = Awaited<ReturnType<typeof undiciFetch>>;

function procedureUrl(procedure: string): string {
  return new URL(`/$procs/${procedure}`, env.SMS_API_URL).toString();
}

/**
 * `POST /$procs/workerLocks` — a snapshot of which node holds which
 * singleton-role advisory lock, right now. No caller-supplied args (the
 * procedure takes none); `{}` as the body, matching every other `$procs`
 * call's envelope (`{ args }`).
 */
export async function workerLocks(): Promise<WorkerLocksResult> {
  const url = procedureUrl("workerLocks");

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
    throw mapGatewayError(response.status, parsed, "workerLocks");
  }
  return parsed as WorkerLocksResult;
}
