// The sealing fetch's own logic, with a sealer that records what it is asked to
// seal. (The real sealer, the real key and a real verifier are in
// `frontends/apps/admin/lib/sealing.test.ts`; this file holds what does not need
// them.)

import { describe, expect, it, vi } from "vitest";
import {
  createSealingFetch,
  etagOf,
  METHOD_HEADER,
  type SealingRuntime,
  STATUS_HEADER,
  type WasmSealedCall,
} from "./sealing-fetch";

function runtime(open: () => Promise<string>) {
  const sealCall = vi.fn(
    async (
      _method: string,
      route: string,
      params: string[],
      _body: string | undefined,
      query: string | undefined,
    ): Promise<WasmSealedCall> => {
      let path = route;
      for (const param of params) path = path.replace(/\{[^}]+\}/, encodeURIComponent(param));
      return {
        body: new Uint8Array([0xd2, 1]),
        headers: [
          ["content-type", 'application/cose; cose-type="cose-sign1"'],
          ["cratestack-contract", "x"],
        ],
        target: query ? `${path}?${query}` : path,
        openResponse: open,
      };
    },
  );
  const value: SealingRuntime = {
    sealer: { sealCall },
    routeOf: (_method, path) => {
      const match = /^\/apps\/([^/]+)$/.exec(path);
      return match
        ? { route: "/apps/{id}", params: [decodeURIComponent(match[1] ?? "")] }
        : { route: path, params: [] };
    },
  };
  return { value, sealCall };
}

describe("etagOf", () => {
  it("rebuilds the validator from the signed body's integer version", () => {
    expect(etagOf('{"id":"a","version":3}')).toBe('"3"');
    expect(etagOf('{"version":0}')).toBe('"0"');
  });

  it("has none for a body without one, a fractional or hostile one, or a list", () => {
    for (const text of [
      '{"id":"a"}',
      '{"version":1.5}',
      '{"version":"3"}',
      '{"items":[]}',
      "[]",
      "",
      "not json",
      '{"version":1e400}',
    ]) {
      expect(etagOf(text), text).toBeUndefined();
    }
  });
});

describe("createSealingFetch", () => {
  it("passes the route, the decoded parameters, the query and the two bound headers to the sealer", async () => {
    const { value, sealCall } = runtime(async () => "{}");
    const send = vi.fn(async () => new Response(new Uint8Array([1]), { status: 200 }));

    await createSealingFetch({ runtime: async () => value, endpoint: "/api/gateway", send })(
      "https://gateway.invalid/apps/a%2Fb?fields=id",
      {
        method: "PATCH",
        headers: { "if-match": '"3"', "idempotency-key": "k1", authorization: "Bearer x" },
        body: '{"active":false}',
      },
    );

    expect(sealCall).toHaveBeenCalledWith(
      "PATCH",
      "/apps/{id}",
      ["a/b"],
      '{"active":false}',
      "fields=id",
      "k1",
      '"3"',
    );
    const [url, init] = send.mock.calls[0] as unknown as [string, RequestInit];
    expect(url).toBe("/api/gateway/apps/a%2Fb?fields=id");
    expect(init.method).toBe("POST");
    expect(init.credentials).toBe("omit");
    expect(init.redirect).toBe("error");
    const names = (init.headers as [string, string][]).map(([name]) => name.toLowerCase());
    expect(names).toContain(METHOD_HEADER);
    // The caller's Authorization never reaches the wire: only what the seal made.
    expect(names).not.toContain("authorization");
  });

  it("believes the status the seal binds, not the one on the carrier", async () => {
    const opened: number[] = [];
    const { value } = runtime(async () => "");
    const sealCall = value.sealer.sealCall;
    value.sealer.sealCall = async (...args) => {
      const sealed = await sealCall(...args);
      return {
        ...sealed,
        openResponse: async (status) => {
          opened.push(status);
          return "";
        },
      };
    };
    const send = async () =>
      new Response(new Uint8Array([1]), { status: 200, headers: { [STATUS_HEADER]: "204" } });

    const response = await createSealingFetch({
      runtime: async () => value,
      endpoint: "/api/gateway",
      send,
    })("https://gateway.invalid/apps/a", { method: "DELETE" });

    expect(opened).toEqual([204]);
    expect(response.status).toBe(204);
    expect(await response.text()).toBe("");
  });

  it("refuses a body that is not JSON text, and never calls the sealer", async () => {
    const { value, sealCall } = runtime(async () => "{}");

    const response = await createSealingFetch({
      runtime: async () => value,
      endpoint: "/api/gateway",
    })("https://gateway.invalid/apps", { method: "POST", body: new Uint8Array([1]) });

    expect(response.status).toBe(400);
    expect(sealCall).not.toHaveBeenCalled();
  });

  it("reports a network failure as unavailable, not as an error with the browser's details", async () => {
    const { value } = runtime(async () => "{}");

    const response = await createSealingFetch({
      runtime: async () => value,
      endpoint: "/api/gateway",
      send: async () => {
        throw new TypeError("Failed to fetch");
      },
    })("https://gateway.invalid/apps");

    expect(response.status).toBe(503);
    expect(await response.text()).not.toContain("Failed to fetch");
  });
});
