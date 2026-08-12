import "server-only";

/**
 * Server-only wiring around `./oidc.ts`'s pure crypto — reads `@vsms/env`
 * and Next's own `cookies()`, neither of which `oidc.ts` may import (see
 * that module's own doc: it has to stay Edge-portable for
 * `admin/middleware.ts`, and `server-only` guards exactly the opposite —
 * "never importable outside a server context," which is fine for a route
 * handler but would make `middleware.ts` itself fail to import this file
 * at all under a stricter enforcement than Next currently applies).
 *
 * Every function here is Node-runtime only (route handlers, Server
 * Components) — `middleware.ts` calls `../lib/oidc.ts` directly instead.
 */

import { env } from "@vsms/env";
import { cookies } from "next/headers";
import {
  decryptSession,
  decryptTxn,
  encryptSession,
  encryptTxn,
  OIDC_COOKIE_NAMES,
  type OidcTxn,
  SESSION_COOKIE_MAX_AGE_SECONDS,
  type Session,
  TXN_COOKIE_MAX_AGE_SECONDS,
} from "./oidc";

const COOKIE_BASE = {
  httpOnly: true,
  secure: env.NODE_ENV === "production",
  sameSite: "lax" as const,
  path: "/",
};

export async function readTxnCookie(): Promise<OidcTxn | undefined> {
  const store = await cookies();
  const token = store.get(OIDC_COOKIE_NAMES.txn)?.value;
  if (token === undefined) return undefined;
  return decryptTxn(token, env.SMS_CONSOLE_SESSION_SECRET);
}

export async function writeTxnCookie(txn: OidcTxn): Promise<void> {
  const store = await cookies();
  const token = await encryptTxn(txn, env.SMS_CONSOLE_SESSION_SECRET);
  store.set(OIDC_COOKIE_NAMES.txn, token, {
    ...COOKIE_BASE,
    maxAge: TXN_COOKIE_MAX_AGE_SECONDS,
  });
}

export async function clearTxnCookie(): Promise<void> {
  const store = await cookies();
  store.delete(OIDC_COOKIE_NAMES.txn);
}

export async function readSession(): Promise<Session | undefined> {
  const store = await cookies();
  const token = store.get(OIDC_COOKIE_NAMES.session)?.value;
  if (token === undefined) return undefined;
  return decryptSession(token, env.SMS_CONSOLE_SESSION_SECRET);
}

export async function writeSessionCookie(session: Session): Promise<void> {
  const store = await cookies();
  const token = await encryptSession(session, env.SMS_CONSOLE_SESSION_SECRET);
  store.set(OIDC_COOKIE_NAMES.session, token, {
    ...COOKIE_BASE,
    maxAge: SESSION_COOKIE_MAX_AGE_SECONDS,
  });
}

export async function clearSessionCookie(): Promise<void> {
  const store = await cookies();
  store.delete(OIDC_COOKIE_NAMES.session);
}
