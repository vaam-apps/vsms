// The pass-through is a pipe, and a pipe is only dumb if nothing in it is allowed
// to be clever. These hold what it may carry and where: nothing it was not
// given a seal for, no credential in either direction, no route that is not a
// sealed one.

import { describe, expect, it, vi } from "vitest";
import { STATUS_HEADER } from "../sealing-fetch";
import {
  COSE_SIGN1,
  createPassThrough,
  FORWARDED_REQUEST_HEADERS,
  MAX_REQUEST_BYTES,
  type RouteTable,
  type Upstream,
  upstreamTarget,
} from "./pass-through";

// A few operations of the schema, standing in for the table the console passes
// (the Node SDK's `routeOf`). The real table is held to the real paths by
// `frontends/apps/admin/lib/pass-through-routes.test.ts`; what is held HERE is
// that the pipe asks it, with the real method and the raw path, and carries
// nothing it does not name.
const OPERATIONS = [
  "GET /apps",
  "GET /apps/{id}",
  "PATCH /apps/{id}",
  "DELETE /apps/{id}",
  "GET /messages",
  "GET /messages/{id}",
  "GET /console_keys",
  "GET /sender_ids",
  "POST /$procs/sendMessage",
];

const serves: RouteTable = (method, path) =>
  OPERATIONS.some((operation) => {
    const [m, template = ""] = operation.split(" ");
    const parts = template.split("/");
    const segments = path.split("/");
    return (
      m === method &&
      parts.length === segments.length &&
      parts.every((part, i) => (part.startsWith("{") && segments[i] !== "") || part === segments[i])
    );
  });

function pipe(upstream: Upstream, table: RouteTable = serves) {
  return createPassThrough({ mount: "/api/gateway", upstream, serves: table });
}

function request(
  path: string,
  init: { method?: string; headers?: Record<string, string>; body?: BodyInit | null } = {},
): Request {
  return new Request(`http://console.test/api/gateway${path}`, {
    method: init.method ?? "POST",
    headers: {
      "content-type": COSE_SIGN1,
      "x-vsms-method": "GET",
      ...init.headers,
    },
    body: init.body === undefined ? new Uint8Array([0xd2, 1, 2, 3]) : init.body,
  });
}

const answered = (status = 200): Upstream =>
  vi.fn(async () => ({
    status,
    headers: new Headers({
      "content-type": COSE_SIGN1,
      "cratestack-payload-type": "application/json",
      "set-cookie": "sid=1",
      location: "https://evil.example/",
      "x-powered-by": "gateway",
      "idempotency-replayed": "true",
    }),
    body: new Uint8Array([0xd2, 9]),
  }));

describe("what is carried upstream", () => {
  it("forwards the sealed bytes, the real method and the path and query, and nothing else", async () => {
    const upstream = answered();

    const response = await pipe(upstream)(
      request("/messages/m1?fields=id", {
        headers: {
          accept: COSE_SIGN1,
          "cratestack-contract": "abc",
          "idempotency-key": "k1",
          "if-match": '"3"',
          authorization: "Bearer stolen",
          cookie: "session=1",
          "x-forwarded-for": "6.6.6.6",
          "x-anything": "else",
        },
      }),
    );

    expect(response.status).toBe(200);
    const [sent] = (upstream as ReturnType<typeof vi.fn>).mock.calls[0] as [
      Parameters<Upstream>[0],
    ];
    expect(sent.method).toBe("GET");
    expect(sent.target).toBe("/messages/m1?fields=id");
    expect(Array.from(sent.body)).toEqual([0xd2, 1, 2, 3]);
    const names = sent.headers.map(([name]) => name);
    expect(names.sort()).toEqual(
      ["accept", "content-type", "cratestack-contract", "idempotency-key", "if-match"].sort(),
    );
    for (const forbidden of ["authorization", "cookie", "x-forwarded-for", "x-anything"]) {
      expect(names).not.toContain(forbidden);
    }
  });

  it("only ever forwards headers from its allow-list: no credential is on it", () => {
    for (const name of FORWARDED_REQUEST_HEADERS) {
      expect(name).not.toMatch(/authorization|cookie|token|secret|x-api-key/i);
    }
  });

  it("carries only a POST of a COSE message, naming one of the four methods", async () => {
    const upstream = answered();
    const p = pipe(upstream);

    expect((await p(request("/apps", { method: "GET", body: null }))).status).toBe(405);
    expect(
      (await p(request("/apps", { headers: { "content-type": "application/json" } }))).status,
    ).toBe(415);
    expect((await p(request("/apps", { headers: { "x-vsms-method": "PUT" } }))).status).toBe(400);
    expect((await p(request("/apps", { headers: { "x-vsms-method": "CONNECT" } }))).status).toBe(
      400,
    );
    expect(upstream).not.toHaveBeenCalled();
  });

  it("is not an open proxy: the unsealed routes and the odd paths are not reachable", async () => {
    const upstream = answered();
    const p = pipe(upstream);

    for (const path of [
      "/healthz",
      "/Healthz",
      "/HEALTHZ",
      "/readyz",
      "/metrics",
      "/token",
      "/jwks.json",
      "/.well-known/openid-configuration",
      "/login",
      "/login/sso/start",
      "/dlr/orange",
      "/oauth/authorize",
      "/messages/../token",
      "/messages/%2e%2e/token",
      "//evil",
      "/a/b/c/d",
      "/",
    ]) {
      expect((await p(request(path))).status, path).toBe(404);
    }
    expect(upstream).not.toHaveBeenCalled();
  });

  it("asks the schema's table, with the real method and the raw path and no query", async () => {
    const table = vi.fn<RouteTable>(() => true);

    await pipe(
      answered(),
      table,
    )(request("/messages/m%2F1?fields=id", { headers: { "x-vsms-method": "PATCH" } }));

    expect(table).toHaveBeenCalledExactlyOnceWith("PATCH", "/messages/m%2F1");
  });

  it("carries a route only under a method the schema serves it under", async () => {
    const upstream = answered();
    const p = pipe(upstream);

    expect(
      (await p(request("/messages/m1", { headers: { "x-vsms-method": "DELETE" } }))).status,
    ).toBe(404);
    expect((await p(request("/sender_ids", { headers: { "x-vsms-method": "POST" } }))).status).toBe(
      404,
    );
    expect(upstream).not.toHaveBeenCalled();
  });

  it("carries nothing the table does not name, whatever the path is called", async () => {
    const upstream = answered();
    const nothing = pipe(upstream, () => false);

    for (const path of ["/apps", "/messages/m1", "/$procs/sendMessage", "/some_new_route"]) {
      expect((await nothing(request(path))).status, path).toBe(404);
    }
    expect(upstream).not.toHaveBeenCalled();
  });

  it("reaches the sealed routes, procedures and encoded ids included", async () => {
    for (const [method, path, target] of [
      ["POST", "/$procs/sendMessage", "/$procs/sendMessage"],
      ["GET", "/messages/m%2F1", "/messages/m%2F1"],
      ["GET", "/console_keys?thumbprint=ab&limit=1", "/console_keys?thumbprint=ab&limit=1"],
      ["GET", "/sender_ids", "/sender_ids"],
    ] as const) {
      const upstream = answered();
      const response = await pipe(upstream)(
        request(path, { headers: { "x-vsms-method": method } }),
      );
      expect(response.status, path).toBe(200);
      expect((upstream as ReturnType<typeof vi.fn>).mock.calls[0]?.[0].target).toBe(target);
    }
  });

  it("refuses a body over the limit, however it is sent", async () => {
    const upstream = answered();
    const big = new Uint8Array(MAX_REQUEST_BYTES + 1);

    const response = await pipe(upstream)(request("/apps", { body: big }));

    expect(response.status).toBe(413);
    expect(upstream).not.toHaveBeenCalled();
  });

  it("refuses a body whose Content-Length already says it is over the limit, without reading it", async () => {
    const upstream = answered();

    const response = await pipe(upstream)(
      request("/apps", { headers: { "content-length": String(MAX_REQUEST_BYTES + 1) } }),
    );

    expect(response.status).toBe(413);
    expect(upstream).not.toHaveBeenCalled();
  });
});

describe("what comes back", () => {
  it("returns the answer's body and status with only the headers a seal is opened with", async () => {
    const response = await pipe(answered(409))(request("/apps"));

    expect(response.status).toBe(409);
    expect(response.headers.get("content-type")).toBe(COSE_SIGN1);
    expect(response.headers.get("cratestack-payload-type")).toBe("application/json");
    expect(response.headers.get("idempotency-replayed")).toBe("true");
    expect(response.headers.get("cache-control")).toBe("no-store");
    expect(response.headers.get("set-cookie")).toBeNull();
    expect(response.headers.get("location")).toBeNull();
    expect(response.headers.get("x-powered-by")).toBeNull();
    expect(Array.from(new Uint8Array(await response.arrayBuffer()))).toEqual([0xd2, 9]);
  });

  it("does not follow or pass on a redirect", async () => {
    const response = await pipe(answered(302))(request("/apps"));

    expect(response.status).toBe(302);
    expect(response.headers.get("location")).toBeNull();
  });

  it("carries a sealed 204 as a 200 and names the real status, since a 204 cannot have the body that signs it", async () => {
    const response = await pipe(answered(204))(
      request("/apps/a1", { headers: { "x-vsms-method": "DELETE" } }),
    );

    expect(response.status).toBe(200);
    expect(response.headers.get(STATUS_HEADER)).toBe("204");
    expect(new Uint8Array(await response.arrayBuffer()).length).toBeGreaterThan(0);
  });

  it("says 502, and nothing more, when the gateway cannot be reached", async () => {
    const response = await pipe(async () => {
      throw new Error("connect ECONNREFUSED 10.0.0.7:8080");
    })(request("/apps"));

    expect(response.status).toBe(502);
    expect(await response.text()).not.toContain("10.0.0.7");
  });
});

describe("upstreamTarget", () => {
  it("keeps the query verbatim and refuses what a URL parser could resolve away", () => {
    expect(upstreamTarget("/messages?x=a?b")).toBe("/messages?x=a?b");
    expect(upstreamTarget("/messages/..")).toBeUndefined();
    expect(upstreamTarget("/messages/.")).toBeUndefined();
    expect(upstreamTarget("/messages/%2E%2E")).toBeUndefined();
    expect(upstreamTarget("/messages/a%2Fb")).toBe("/messages/a%2Fb");
    expect(upstreamTarget("/messages/a%5Cb")).toBeUndefined();
    expect(upstreamTarget("healthz")).toBeUndefined();
  });
});
