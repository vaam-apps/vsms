// The console's view of its own key: `GET /console_keys` (the caller's rows),
// `POST /$procs/revokeConsoleKey`, and the cheap sealed read that tells a
// browser whether the gateway knows its key yet.
//
// `ConsoleKey` is written by `registerConsoleKey` (vaam's pairing client, once a
// vaam admin phone approves) and by `sms-gateway pair-console-key` (the
// break-glass), never by the console. The console only reads its own row and
// revokes it. `docs/design/console-pairing.md` has the flow; the policy is
// `ConsoleKey`'s `@@allow("read")`: owner and admin see every key, anyone else
// only their own, and `revokeConsoleKey` applies the same read policy to
// `args.keyId`, so revoking is "whoever can see it".

import { COMPOSE_PROOF_PATH, COMPOSE_PROOF_QUERY } from "./compose-proof";
import { mapGatewayError } from "./errors";
import { parseGatewayJson } from "./json";
import { postJson } from "./rest";
import { GATEWAY_ORIGIN, gatewayFetch } from "./upstream";

export interface ConsoleKeyRecord {
  id: string;
  userId: string;
  /** Lowercase hex, 64 characters: the RFC 9679 COSE key thumbprint. */
  thumbprint: string;
  label?: string | undefined;
  pairedAt: string;
  /** `pairedAt` + 30 days, set once and never extended. */
  expiresAt: string;
  revokedAt?: string | undefined;
}

const KEY_FIELDS = ["id", "userId", "thumbprint", "label", "pairedAt", "expiresAt", "revokedAt"];

/** Whether the gateway accepts this browser's key: one sealed read that any
 *  paired console may make. Resolves when the gateway answered 200; rejects with
 *  the `GatewayError` it maps to otherwise, whose `httpStatus` 401 is "not
 *  paired (yet)" and nothing worse. */
export async function probeKey(): Promise<void> {
  const url = new URL(COMPOSE_PROOF_PATH, GATEWAY_ORIGIN);
  url.search = COMPOSE_PROOF_QUERY;
  const response = await gatewayFetch(url.toString(), {
    method: "GET",
    headers: { accept: "application/json" },
  });
  if (!response.ok) {
    throw mapGatewayError(response.status, await parseGatewayJson(response), "probeKey");
  }
}

/** The row for the key with `thumbprint`, or `null` when the caller may not see
 *  one. Only meaningful after {@link probeKey} succeeded. */
export async function getConsoleKeyByThumbprint(
  thumbprint: string,
): Promise<ConsoleKeyRecord | null> {
  const url = new URL("/console_keys", GATEWAY_ORIGIN);
  url.searchParams.set("thumbprint", thumbprint);
  url.searchParams.set("limit", "1");
  url.searchParams.set("fields", KEY_FIELDS.join(","));
  const response = await gatewayFetch(url.toString(), {
    method: "GET",
    headers: { accept: "application/json" },
  });
  const parsed = await parseGatewayJson(response);
  if (!response.ok) throw mapGatewayError(response.status, parsed, "getConsoleKey");
  const page = parsed as { items?: ConsoleKeyRecord[] };
  return page.items?.[0] ?? null;
}

/** Revokes a key. Idempotent (`revokedAt` keeps its first value), and the row
 *  stays: a revoked thumbprint cannot be registered again. */
export async function revokeConsoleKey(keyId: string): Promise<ConsoleKeyRecord> {
  return postJson<ConsoleKeyRecord>(
    "/$procs/revokeConsoleKey",
    { args: { keyId } },
    "revokeConsoleKey",
  );
}
