// The console's pass-through to the gateway: sealed bytes in, sealed bytes out.
//
// Holds no credential and no session (`@vsms/gateway/bff`, `pass-through.ts`).
// The request is already signed by the browser's own key; the gateway
// authenticates the signature, not this connection, and this handler decides
// only what bytes may travel and where. `runtime = "nodejs"`: the gateway call
// reads mTLS material from disk and uses `node:https`, which the Edge runtime
// has neither of. `dynamic = "force-dynamic"`: an answer is never cacheable.
//
// What it will carry is what the schema serves: `routeOf` is the table the
// browser's signature is bound against, so a path the gateway does not seal
// (health, metrics, delivery receipts) is not in it
// and cannot be reached through here.

import { routeOf } from "@vaam-apps/vsms-node";
import { createPassThrough } from "@vsms/gateway/bff";
import { gatewayUpstream } from "@/lib/server/gateway-address";

export const runtime = "nodejs";
export const dynamic = "force-dynamic";

export function POST(request: Request): Promise<Response> {
  return createPassThrough({
    mount: "/api/gateway",
    upstream: gatewayUpstream(),
    serves: (method, path) => routeOf(method, path) !== undefined,
  })(request);
}
