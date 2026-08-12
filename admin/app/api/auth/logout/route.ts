/**
 * `POST /api/auth/logout` (#194). Clears the session cookie outright —
 * `authkestra-op` has no `/revoke` endpoint (§5.3 of the design doc notes
 * this is a real, standing gap for the machine-credential side too), so
 * the refresh token this session may still hold stays valid at the OP
 * until it naturally expires; dropping the cookie is what actually ends
 * *this browser's* session, matching #48's own pre-#194 "does not
 * provide... revocation" framing for the equivalent gap on that side.
 */
import "server-only";

import { redirect } from "next/navigation";
import { clearSessionCookie } from "../../../../lib/session";

export const runtime = "nodejs";

export async function POST(): Promise<Response> {
  await clearSessionCookie();
  redirect("/login");
}
