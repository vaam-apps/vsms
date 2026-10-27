// The pass-through carries what the schema serves and nothing else, held against
// the REAL table: the wasm module's `routeOf`, the one the browser's signature
// is bound with and the console's route hands the pass-through. A table kept
// beside the pipe, or a list of what not to carry, could drift from what the
// gateway actually seals; this one is the same data the sealer signs against.

import { COSE_SIGN1, createPassThrough } from "@vsms/gateway/bff";
import { describe, expect, it, vi } from "vitest";
import { loadTestModule } from "./test-support/wasm";

async function pipe() {
  const module = await loadTestModule();
  const upstream = vi.fn(async () => ({
    status: 200,
    headers: new Headers(),
    body: new Uint8Array([1]),
  }));
  const serves = (method: string, path: string) => {
    try {
      module.routeOf(method, path);
      return true;
    } catch {
      return false;
    }
  };
  return { upstream, handle: createPassThrough({ mount: "/api/gateway", upstream, serves }) };
}

function post(path: string, method: string): Request {
  return new Request(`http://console.test/api/gateway${path}`, {
    method: "POST",
    headers: { "content-type": COSE_SIGN1, "x-vsms-method": method },
    body: new Uint8Array([0xd2]),
  });
}

describe("the pass-through serves the schema's operations", () => {
  it("carries every kind of operation the console sends", async () => {
    const { handle, upstream } = await pipe();
    for (const [method, path] of [
      ["GET", "/apps?limit=500"],
      ["GET", "/messages/m%2F1"],
      ["PATCH", "/providers/p1"],
      ["DELETE", "/apps/a1"],
      ["POST", "/$procs/sendMessage"],
      ["POST", "/$procs/authorizeComposition"],
      ["GET", "/sender_ids"],
    ] as const) {
      expect((await handle(post(path, method))).status, `${method} ${path}`).toBe(200);
    }
    expect(upstream).toHaveBeenCalledTimes(7);
  });

  it("refuses every route the gateway does not seal, and a made-up one, without opening a connection", async () => {
    const { handle, upstream } = await pipe();
    for (const [method, path] of [
      ["GET", "/metrics"],
      ["GET", "/healthz"],
      ["GET", "/readyz"],
      ["GET", "/health"],
      ["POST", "/login"],
      ["POST", "/token"],
      ["GET", "/jwks.json"],
      ["POST", "/dlr/orange_cm"],
      ["GET", "/login/sso/start"],
      ["GET", "/some_route_the_schema_never_had"],
    ] as const) {
      expect((await handle(post(path, method))).status, `${method} ${path}`).toBe(404);
    }
    expect(upstream).not.toHaveBeenCalled();
  });

  it("refuses a real path under a method the schema does not serve it under", async () => {
    const { handle, upstream } = await pipe();
    for (const [method, path] of [
      ["DELETE", "/$procs/sendMessage"],
      ["PATCH", "/$procs/sendMessage"],
      ["GET", "/$procs/sendMessage"],
    ] as const) {
      expect((await handle(post(path, method))).status, `${method} ${path}`).toBe(404);
    }
    expect(upstream).not.toHaveBeenCalled();
  });
});
