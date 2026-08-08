/**
 * The dashboard's authentication gate — `none` or `basic`, no database.
 *
 * This exists because `/api/trpc` is not a read-only surface: `compose.send`
 * triggers a real `sendMessage` on the gateway, which sends a real SMS on the
 * console's machine credential and costs real money. Until this middleware
 * landed, `DASHBOARD_AUTH` was validated at boot by `@vsms/env` but nothing
 * consumed it, so a deployment could pass env validation and still serve that
 * endpoint to anyone who could reach the server.
 *
 * # What this does NOT provide
 *
 * Deliberate scope, and it must be stated rather than assumed:
 *
 * - **No identity at the gateway.** Basic-auth users are not `User` rows and
 *   carry no role. Everything upstream uses one machine credential, so
 *   `cratestack_audit` attributes every write to `SMS_CONSOLE_CLIENT_ID`, not
 *   to a person.
 * - **No roles or per-user permissions.** Every authenticated user reaches
 *   every screen the machine token can reach.
 * - **No logout, revocation, lockout, or rate limiting.** Rotating
 *   `DASHBOARD_BASIC_USERS` and restarting is the whole revocation story;
 *   brute-force protection belongs at the reverse proxy.
 * - **SHA-256 is not a password hash.** It is fast by design and offline
 *   crackable from a leaked env. Acceptable only as an internal dev switch
 *   behind a network allowlist; it is not a production human-auth story.
 *
 * Runs on the Edge runtime, which is why the digest is `crypto.subtle`
 * SHA-256 rather than a real KDF — no Node APIs, no `Buffer`, no bcrypt.
 */
import { type NextRequest, NextResponse } from "next/server";

const ACTOR_HEADER = "x-vsms-actor";

/**
 * `/api/trpc` is included on purpose. Protecting pages while leaving the RPC
 * endpoint open is the classic version of this bug, and it is the endpoint
 * that actually sends messages.
 */
export const config = {
  matcher: ["/((?!_next/static|_next/image|favicon.ico|icons/|manifest.webmanifest|sw.js).*)"],
};

function unauthorized(realm: string): NextResponse {
  return new NextResponse("Unauthorized", {
    status: 401,
    headers: { "WWW-Authenticate": `Basic realm="${realm}", charset="UTF-8"` },
  });
}

/** Constant-time compare over two equal-length hex digests. */
function digestsMatch(a: string, b: string): boolean {
  if (a.length !== b.length) return false;
  let diff = 0;
  for (let i = 0; i < a.length; i++) {
    diff |= a.charCodeAt(i) ^ b.charCodeAt(i);
  }
  return diff === 0;
}

async function sha256Hex(value: string): Promise<string> {
  const bytes = new TextEncoder().encode(value);
  const digest = await crypto.subtle.digest("SHA-256", bytes);
  return Array.from(new Uint8Array(digest))
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
}

function parseUsers(raw: string): Map<string, string> {
  const users = new Map<string, string>();
  for (const entry of raw.split(",")) {
    const trimmed = entry.trim();
    if (trimmed === "") continue;
    const colon = trimmed.indexOf(":");
    if (colon === -1) continue;
    users.set(trimmed.slice(0, colon), trimmed.slice(colon + 1).toLowerCase());
  }
  return users;
}

export async function middleware(request: NextRequest): Promise<NextResponse> {
  // Strip any inbound actor header BEFORE anything else. Downstream code
  // trusts `x-vsms-actor`, so a caller that could set it would be asserting
  // whatever identity it liked.
  const headers = new Headers(request.headers);
  headers.delete(ACTOR_HEADER);

  const mode = process.env.DASHBOARD_AUTH ?? "none";
  const realm = process.env.DASHBOARD_BASIC_REALM ?? "vsms admin";

  if (mode !== "basic") {
    headers.set(ACTOR_HEADER, "anonymous");
    return NextResponse.next({ request: { headers } });
  }

  const rawUsers = process.env.DASHBOARD_BASIC_USERS ?? "";
  const users = parseUsers(rawUsers);
  if (users.size === 0) return unauthorized(realm);

  const authorization = request.headers.get("authorization");
  if (authorization === null || !authorization.startsWith("Basic ")) {
    return unauthorized(realm);
  }

  let decoded: string;
  try {
    decoded = atob(authorization.slice("Basic ".length).trim());
  } catch {
    return unauthorized(realm);
  }

  // Split on the FIRST colon only — passwords legitimately contain colons.
  const colon = decoded.indexOf(":");
  if (colon === -1) return unauthorized(realm);
  const user = decoded.slice(0, colon);
  const password = decoded.slice(colon + 1);

  // Always hash, and always compare against a fixed-length digest, so an
  // unknown username costs the same as a wrong password.
  const presented = await sha256Hex(password);
  const expected = users.get(user) ?? "0".repeat(64);
  if (!digestsMatch(presented, expected) || !users.has(user)) {
    return unauthorized(realm);
  }

  headers.set(ACTOR_HEADER, user);
  return NextResponse.next({ request: { headers } });
}
