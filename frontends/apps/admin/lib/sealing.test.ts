// The console's data layer, end to end through the REAL sealing path: the real
// data-layer functions, the real sealing `fetch`, the real wasm sealer signing
// with a real non-extractable WebCrypto key, against a gateway that is a real
// sealed verifier (cratestack's server envelope, sharing no code with the
// sealer). Only the network is replaced.
//
// What this holds, in the order it matters:
//   * every call leaves sealed and carries no credential of any kind;
//   * an answer is believed only if it verifies, for THIS request, under one of
//     the two pinned keys: plain JSON, a forgery, another request's answer and
//     an answer under an unpinned key are all refused;
//   * the route that was signed is the route that was asked for.

import {
  configureComposeFetch,
  configureGatewayFetch,
  createApp,
  createSealingFetch,
  deleteApp,
  GatewayError,
  getAppById,
  getConsoleKeyByThumbprint,
  isStaleWriteError,
  listApps,
  listJobs,
  NoDeviceKeyError,
  probeKey,
  revokeConsoleKey,
  type SealingRuntime,
  updateApp,
} from "@vsms/gateway";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { type FakeGateway, startFakeGateway } from "./test-support/fake-gateway";

const COSE_SIGN1 = 'application/cose; cose-type="cose-sign1"';

/** What may accompany a sealed body: the headers a seal depends on, the real
 *  method for the pass-through, and nothing else. */
const ALLOWED_HEADERS = new Set([
  "content-type",
  "accept",
  "cratestack-contract",
  "cratestack-payload-type",
  "cratestack-payload-accept",
  "idempotency-key",
  "if-match",
  "x-vsms-method",
]);

function install(gateway: FakeGateway, onUnauthenticated?: () => void) {
  configureGatewayFetch(
    createSealingFetch({
      runtime: async () => gateway.runtime,
      endpoint: "/api/gateway",
      send: gateway.send,
      ...(onUnauthenticated ? { onUnauthenticated } : {}),
    }),
  );
}

afterEach(() => {
  configureGatewayFetch(undefined);
  configureComposeFetch(undefined);
});

const app = (version: number) => ({
  id: "app1",
  name: "Acme",
  slug: "acme",
  monthlyQuota: 1000,
  transliterateToGsm7: true,
  active: true,
  ipAllowlist: " ",
  version,
  createdAt: "2026-01-01T00:00:00Z",
  updatedAt: "2026-01-01T00:00:00Z",
});

describe("every outbound call is sealed and carries no credential", () => {
  let gateway: FakeGateway;

  beforeAll(async () => {
    gateway = await startFakeGateway({
      route: ({ method, path }) => {
        if (method === "GET" && path === "/apps")
          return {
            status: 200,
            body: JSON.stringify({ items: [app(1)], totalCount: 1, pageInfo: {} }),
          };
        if (method === "GET" && path.startsWith("/apps/"))
          return { status: 200, body: JSON.stringify(app(3)) };
        if (method === "PATCH") return { status: 200, body: JSON.stringify(app(4)) };
        if (method === "DELETE") return { status: 204, body: "" };
        if (method === "POST" && path === "/apps")
          return { status: 201, body: JSON.stringify(app(1)) };
        if (path === "/console_keys")
          return { status: 200, body: JSON.stringify({ items: [{ id: "ck1", userId: "u1" }] }) };
        if (path === "/$procs/revokeConsoleKey")
          return { status: 200, body: JSON.stringify({ id: "ck1", userId: "u1" }) };
        if (path === "/jobs")
          return { status: 200, body: JSON.stringify({ items: [], totalCount: 0 }) };
        if (path === "/sender_ids") return { status: 200, body: JSON.stringify({ items: [] }) };
        return { status: 404, body: JSON.stringify({ code: "NOT_FOUND", message: "no" }) };
      },
    });
  });

  it("through the real data layer, for a read, a write under If-Match, a delete and a procedure", async () => {
    install(gateway);
    gateway.requests.length = 0;

    await listApps();
    const found = await getAppById("app1");
    await updateApp("app1", found?.etag ?? "", { active: false });
    await deleteApp("app1", "4");
    await createApp({ name: "Acme", slug: "acme", monthlyQuota: 10 } as never);
    await listJobs({} as never);
    await probeKey();
    await getConsoleKeyByThumbprint("ab".repeat(32));
    await revokeConsoleKey("ck1");

    // Nine calls, nine sealed requests, none dropped on the way.
    expect(gateway.requests).toHaveLength(9);
    for (const request of gateway.requests) {
      const label = `${request.method} ${request.url}`;
      expect(request.url, label).toMatch(/^\/api\/gateway\//);
      expect(request.headers["content-type"], label).toBe(COSE_SIGN1);
      // The body is a COSE message: CBOR tag 18 (COSE_Sign1), not JSON.
      expect(request.body[0], label).toBe(0xd2);
      expect(
        Object.keys(request.headers).filter((h) => !ALLOWED_HEADERS.has(h)),
        label,
      ).toEqual([]);
      expect(request.headers.authorization, label).toBeUndefined();
      expect(request.headers.cookie, label).toBeUndefined();
      expect(request.credentials, label).toBe("omit");
    }
    expect(gateway.requests.map((r) => r.method)).toEqual([
      "GET",
      "GET",
      "PATCH",
      "DELETE",
      "POST",
      "GET",
      "GET",
      "GET",
      "POST",
    ]);
  });

  it("reaches the gateway only as requests the gateway accepted: the verifier saw each one", async () => {
    const seen: string[] = [];
    const watched = await startFakeGateway({
      route: ({ method, path }) => {
        seen.push(`${method} ${path}`);
        return { status: 200, body: JSON.stringify({ items: [], totalCount: 0, pageInfo: {} }) };
      },
    });
    install(watched);

    await listApps();
    await probeKey();

    expect(seen).toEqual(["GET /apps", "GET /sender_ids"]);
  });

  it("binds the If-Match it sends, and a stale one comes back as a 412 the screen can name", async () => {
    const seenIfMatch: (string | undefined)[] = [];
    const gw = await startFakeGateway({
      route: ({ method, ifMatch }) => {
        seenIfMatch.push(ifMatch);
        return method === "PATCH"
          ? { status: 412, body: JSON.stringify({ code: "PRECONDITION_FAILED", message: "stale" }) }
          : { status: 200, body: JSON.stringify(app(7)) };
      },
    });
    install(gw);

    const fetched = await getAppById("app1");
    expect(fetched?.etag).toBe('"7"');
    const error = await updateApp("app1", fetched?.etag ?? "", { active: false }).catch(
      (e: unknown) => e,
    );

    expect(isStaleWriteError(error)).toBe(true);
    expect(seenIfMatch).toEqual([undefined, '"7"']);
  });

  it("takes the ETag from the signed body, not from a header nobody signed", async () => {
    const gw = await startFakeGateway({
      route: () => ({ status: 200, body: JSON.stringify(app(9)) }),
    });
    install(gw);
    const innocent = gw.send;
    gw.send = (async (input: RequestInfo | URL, init?: RequestInit) => {
      const answer = await innocent(input, init);
      const headers = new Headers(answer.headers);
      headers.set("etag", '"1"');
      return new Response(await answer.arrayBuffer(), { status: answer.status, headers });
    }) as typeof fetch;
    configureGatewayFetch(
      createSealingFetch({
        runtime: async () => gw.runtime,
        endpoint: "/api/gateway",
        send: gw.send,
      }),
    );

    expect((await getAppById("app1"))?.etag).toBe('"9"');
  });
});

describe("an answer is believed only if it verifies", () => {
  it("refuses plain JSON, whatever its status, and never returns what it said", async () => {
    const gateway = await startFakeGateway();
    install(gateway);
    gateway.override = () =>
      new Response(JSON.stringify({ items: [{ id: "forged", name: "Evil" }], totalCount: 1 }), {
        status: 200,
        headers: { "content-type": "application/json" },
      });

    const error = await listApps().catch((e: unknown) => e);

    expect(error).toBeInstanceOf(GatewayError);
    expect((error as GatewayError).httpStatus).toBe(503);
    expect(String(error)).not.toContain("Evil");
  });

  it("refuses an answer dressed as sealed that is not", async () => {
    const gateway = await startFakeGateway();
    install(gateway);
    gateway.override = () =>
      new Response(new Uint8Array([0xd2, 0x84, 0x40, 0xa0, 0x41, 0x00, 0x40]), {
        status: 200,
        headers: { "content-type": COSE_SIGN1, "cratestack-payload-type": "application/json" },
      });

    const error = await listApps().catch((e: unknown) => e);

    expect(error).toBeInstanceOf(GatewayError);
    expect([502, 503]).toContain((error as GatewayError).httpStatus);
  });

  it("refuses a real answer to a different request: an answer is bound to its request", async () => {
    const gateway = await startFakeGateway({
      route: ({ path }) => ({
        status: 200,
        body: JSON.stringify(
          path === "/apps" ? { items: [app(1)], totalCount: 1, pageInfo: {} } : { items: [] },
        ),
      }),
    });
    install(gateway);
    let captured: Response | undefined;
    const real = gateway.send;
    gateway.send = (async (input: RequestInfo | URL, init?: RequestInit) => {
      const answer = await real(input, init);
      captured ??= answer.clone();
      return answer;
    }) as typeof fetch;
    configureGatewayFetch(
      createSealingFetch({
        runtime: async () => gateway.runtime,
        endpoint: "/api/gateway",
        send: gateway.send,
      }),
    );
    await listApps();

    // A hop replays the first answer to the next, different, request.
    gateway.override = () => captured?.clone();
    const error = await probeKey().catch((e: unknown) => e);

    expect(error).toBeInstanceOf(GatewayError);
    expect((error as GatewayError).httpStatus).toBe(502);
  });

  it("refuses an answer sealed under a key that is not pinned", async () => {
    const pinned = await startFakeGateway();
    const impostor = await startFakeGateway({ key: pinned.key });
    // The runtime pins `pinned`'s keys; the answer comes from `impostor`, which
    // accepts the same console key but signs with keys the console never pinned.
    configureGatewayFetch(
      createSealingFetch({
        runtime: async () => pinned.runtime,
        endpoint: "/api/gateway",
        send: impostor.send,
      }),
    );

    const error = await listApps().catch((e: unknown) => e);

    expect(error).toBeInstanceOf(GatewayError);
    expect((error as GatewayError).httpStatus).toBe(502);
  });

  it("reads the gateway's unsigned 401 as 'not paired', says so once, and does not retry", async () => {
    const gateway = await startFakeGateway({ registered: false });
    const unauthenticated = vi.fn();
    install(gateway, unauthenticated);

    const error = await listApps().catch((e: unknown) => e);

    expect(error).toBeInstanceOf(GatewayError);
    expect((error as GatewayError).httpStatus).toBe(401);
    expect((error as GatewayError).trpcCode).toBe("UNAUTHORIZED");
    expect(unauthenticated).toHaveBeenCalledTimes(1);
    expect(gateway.requests).toHaveLength(1);
  });

  it("opens a sealed error the gateway signed, with the status it signed it under", async () => {
    const gateway = await startFakeGateway({
      route: () => ({
        status: 403,
        body: JSON.stringify({ code: "FORBIDDEN", message: "not yours" }),
      }),
    });
    install(gateway);

    const error = await listApps().catch((e: unknown) => e);

    expect((error as GatewayError).httpStatus).toBe(403);
    expect((error as GatewayError).message).toBe("not yours");
  });
});

describe("what is signed is what was asked for", () => {
  it("sends nothing for a route the schema does not serve", async () => {
    const gateway = await startFakeGateway();
    install(gateway);

    const response = await createSealingFetch({
      runtime: async () => gateway.runtime,
      endpoint: "/api/gateway",
      send: gateway.send,
    })("https://gateway.invalid/nope");

    expect(response.status).toBe(400);
    expect(gateway.requests).toHaveLength(0);
  });

  it("sends nothing when the key signed another route than the one asked for", async () => {
    const gateway = await startFakeGateway();
    const lying: SealingRuntime = {
      sealer: gateway.runtime.sealer,
      routeOf: () => ({ route: "/apps/{id}", params: ["someone-else"] }),
    };

    const response = await createSealingFetch({
      runtime: async () => lying,
      endpoint: "/api/gateway",
      send: gateway.send,
    })("https://gateway.invalid/apps/mine");

    expect(response.status).toBe(400);
    expect(gateway.requests).toHaveLength(0);
  });

  it("refuses an origin that is not the gateway's placeholder, and any other method", async () => {
    const gateway = await startFakeGateway();
    const sealingFetch = createSealingFetch({
      runtime: async () => gateway.runtime,
      endpoint: "/api/gateway",
      send: gateway.send,
    });

    expect((await sealingFetch("https://example.com/apps")).status).toBe(400);
    expect(
      (await sealingFetch("https://gateway.invalid/apps", { method: "PUT", body: "{}" })).status,
    ).toBe(400);
    expect(gateway.requests).toHaveLength(0);
  });

  it("sends nothing from a browser that has no key", async () => {
    const gateway = await startFakeGateway();
    const unauthenticated = vi.fn();

    const response = await createSealingFetch({
      runtime: async () => {
        throw new NoDeviceKeyError();
      },
      endpoint: "/api/gateway",
      send: gateway.send,
      onUnauthenticated: unauthenticated,
    })("https://gateway.invalid/apps");

    expect(response.status).toBe(401);
    expect(gateway.requests).toHaveLength(0);
    expect(unauthenticated).toHaveBeenCalledTimes(1);
  });
});
