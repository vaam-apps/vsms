// #59's mechanism-level proof for the web half of "thread ETag / If-Match
// through every edit ... in the data layer once." The genuine end-to-end
// proof — a real Postgres, the real generated CAS SQL, a real
// `CoolError::PreconditionFailed` mapped to a real HTTP 412 — is
// `crates/sms-api/tests/if_match_live_postgres.rs`; that test's own module
// doc explains why *this* file stands in a fake upstream rather than a live
// `sms-gateway` (`GatewayAuth` hardcodes `role: "app"` for every real
// token, so the console's own credential cannot reach a `PATCH
// /providers/{id}`-shaped route today regardless of what this package
// sends). What's real here: an actual `Response`-shaped object with actual
// headers, actual `If-Match` attachment, actual status-based branching —
// `rest.ts`'s own logic, not a reimplementation of it.
//
// `token.ts` is mocked (real key-loading/JWT-signing machinery has nothing
// to do with what this file tests) — `dispatcher.ts` is not; `gatewayAgent()`
// runs for real against the fake `http://sms-api.test` base URL
// `vitest.config.ts` sets, taking the no-certs `http:` branch, exactly the
// way it would in a real dev deployment.

import { describe, expect, it, vi } from "vitest";
import { GatewayError, isStaleWriteError } from "./errors";
import { deleteResource, fetchWithEtag, postJson, updateWithIfMatch } from "./rest";

vi.mock("./token", () => ({
  getAccessToken: vi.fn().mockResolvedValue("test-access-token"),
  invalidateAccessToken: vi.fn(),
}));

interface FakeProvider {
  id: string;
  displayName: string;
  version: number;
}

function jsonResponse(
  status: number,
  body: unknown,
  headers: Record<string, string> = {},
): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json", ...headers },
  });
}

describe("fetchWithEtag", () => {
  it("captures the ETag header alongside the parsed body on a real 200", async () => {
    const provider: FakeProvider = { id: "p1", displayName: "Orange", version: 3 };
    const fetcher = vi.fn().mockResolvedValue(jsonResponse(200, provider, { etag: '"3"' }));

    const result = await fetchWithEtag<FakeProvider>("/providers/p1", "getProvider", fetcher);

    expect(result).not.toBeNull();
    expect(result?.data).toEqual(provider);
    // The raw header value, quotes included — exactly what a real `If-Match`
    // request needs to echo back; this function's job is to capture it
    // verbatim, not to parse `"3"` into `3`.
    expect(result?.etag).toBe('"3"');
    expect(fetcher).toHaveBeenCalledTimes(1);
    const [url, init] = fetcher.mock.calls[0] as [string, RequestInit];
    expect(url).toBe("http://sms-api.test/providers/p1");
    expect((init.headers as Record<string, string>).authorization).toBe("Bearer test-access-token");
  });

  it("returns undefined etag, not a missing field, when the model carries no @version", async () => {
    const fetcher = vi.fn().mockResolvedValue(jsonResponse(200, { id: "x" }));
    const result = await fetchWithEtag<{ id: string }>("/routes/x", "getRoute", fetcher);
    expect(result?.etag).toBeUndefined();
  });

  it("returns null on a 404, matching messages.ts's own getJson convention", async () => {
    const fetcher = vi
      .fn()
      .mockResolvedValue(jsonResponse(404, { code: "NOT_FOUND", message: "no" }));
    const result = await fetchWithEtag<FakeProvider>("/providers/gone", "getProvider", fetcher);
    expect(result).toBeNull();
  });

  it("retries exactly once, with a fresh token, on an unexpected 401", async () => {
    const provider: FakeProvider = { id: "p1", displayName: "Orange", version: 1 };
    const fetcher = vi
      .fn()
      .mockResolvedValueOnce(jsonResponse(401, { code: "UNAUTHORIZED", message: "expired" }))
      .mockResolvedValueOnce(jsonResponse(200, provider, { etag: '"1"' }));

    const result = await fetchWithEtag<FakeProvider>("/providers/p1", "getProvider", fetcher);

    expect(fetcher).toHaveBeenCalledTimes(2);
    expect(result?.data).toEqual(provider);
  });
});

describe("updateWithIfMatch", () => {
  it("sends the caller-supplied etag verbatim as If-Match", async () => {
    const updated: FakeProvider = { id: "p1", displayName: "Renamed", version: 2 };
    const fetcher = vi.fn().mockResolvedValue(jsonResponse(200, updated, { etag: '"2"' }));

    const result = await updateWithIfMatch<FakeProvider>(
      "/providers/p1",
      { displayName: "Renamed" },
      '"1"',
      "updateProvider",
      fetcher,
    );

    expect(result.data).toEqual(updated);
    expect(result.etag).toBe('"2"');
    const [, init] = fetcher.mock.calls[0] as [string, RequestInit];
    expect(init.method).toBe("PATCH");
    expect((init.headers as Record<string, string>)["if-match"]).toBe('"1"');
    expect(JSON.parse(init.body as string)).toEqual({ displayName: "Renamed" });
  });

  /**
   * The actual guard this whole file exists to prove: a stale `If-Match`
   * must surface as a `GatewayError` a screen can specifically recognise as
   * "reload and retry", not fold into the same bucket a generic 500 or a
   * duplicate-key 409 would land in.
   */
  it("a stale If-Match surfaces as a recognisably-stale GatewayError, not a generic one", async () => {
    const fetcher = vi.fn().mockResolvedValue(
      jsonResponse(412, {
        code: "PRECONDITION_FAILED",
        message: "precondition failed: version mismatch",
      }),
    );

    const attempt = updateWithIfMatch<FakeProvider>(
      "/providers/p1",
      { displayName: "Renamed by a stale tab" },
      '"0"',
      "updateProvider",
      fetcher,
    );

    await expect(attempt).rejects.toThrow(GatewayError);
    try {
      await attempt;
      expect.unreachable("updateWithIfMatch must throw on a 412");
    } catch (error) {
      expect(isStaleWriteError(error)).toBe(true);
      expect(error).toBeInstanceOf(GatewayError);
      const gatewayError = error as GatewayError;
      expect(gatewayError.httpStatus).toBe(412);
      expect(gatewayError.gatewayCode).toBe("PRECONDITION_FAILED");
      // Distinct from a generic conflict — the actual claim this function
      // (and `isStaleWriteError`) exists to make true.
      expect(
        isStaleWriteError(new GatewayError("dup", { httpStatus: 409, trpcCode: "CONFLICT" })),
      ).toBe(false);
    }
  });

  it("a genuine 500 is not mistaken for a stale write", async () => {
    const fetcher = vi
      .fn()
      .mockResolvedValue(jsonResponse(500, { code: "INTERNAL_ERROR", message: "boom" }));

    try {
      await updateWithIfMatch<FakeProvider>("/providers/p1", {}, '"1"', "updateProvider", fetcher);
      expect.unreachable("updateWithIfMatch must throw on a 500");
    } catch (error) {
      expect(isStaleWriteError(error)).toBe(false);
      expect(error).toBeInstanceOf(GatewayError);
      expect((error as GatewayError).httpStatus).toBe(500);
    }
  });
});

interface FakeRoute {
  id: string;
  name: string;
  version: number;
}

describe("postJson", () => {
  it("POSTs the given body and returns the parsed response, no ETag handling at all", async () => {
    const created: FakeRoute = { id: "r1", name: "catch-all", version: 0 };
    const fetcher = vi.fn().mockResolvedValue(jsonResponse(200, created, { etag: '"0"' }));

    const result = await postJson<FakeRoute>(
      "/routes",
      { name: "catch-all", priority: 0, weight: 1, enabled: true, providerId: "p1" },
      "createRoute",
      fetcher,
    );

    expect(result).toEqual(created);
    const [url, init] = fetcher.mock.calls[0] as [string, RequestInit];
    expect(url).toBe("http://sms-api.test/routes");
    expect(init.method).toBe("POST");
    expect((init.headers as Record<string, string>).authorization).toBe("Bearer test-access-token");
    expect(JSON.parse(init.body as string)).toEqual({
      name: "catch-all",
      priority: 0,
      weight: 1,
      enabled: true,
      providerId: "p1",
    });
  });

  it("retries exactly once, with a fresh token, on an unexpected 401", async () => {
    const created: FakeRoute = { id: "r1", name: "catch-all", version: 0 };
    const fetcher = vi
      .fn()
      .mockResolvedValueOnce(jsonResponse(401, { code: "UNAUTHORIZED", message: "expired" }))
      .mockResolvedValueOnce(jsonResponse(200, created));

    const result = await postJson<FakeRoute>("/routes", {}, "createRoute", fetcher);

    expect(fetcher).toHaveBeenCalledTimes(2);
    expect(result).toEqual(created);
  });

  it("a validation failure surfaces as a GatewayError carrying field errors", async () => {
    const fetcher = vi.fn().mockResolvedValue(
      jsonResponse(422, {
        code: "VALIDATION_ERROR",
        message: "invalid input",
        details: { priority: ["must be between 0 and 1000"] },
      }),
    );

    try {
      await postJson<FakeRoute>("/routes", { priority: -1 }, "createRoute", fetcher);
      expect.unreachable("postJson must throw on a 422");
    } catch (error) {
      expect(error).toBeInstanceOf(GatewayError);
      const gatewayError = error as GatewayError;
      expect(gatewayError.trpcCode).toBe("BAD_REQUEST");
      expect(gatewayError.fieldErrors?.priority).toEqual(["must be between 0 and 1000"]);
    }
  });
});

describe("deleteResource", () => {
  it("sends a plain DELETE with no body and no If-Match header at all", async () => {
    const fetcher = vi.fn().mockResolvedValue(new Response(null, { status: 204 }));

    await deleteResource("/routes/r1", "deleteRoute", fetcher);

    expect(fetcher).toHaveBeenCalledTimes(1);
    const [url, init] = fetcher.mock.calls[0] as [string, RequestInit];
    expect(url).toBe("http://sms-api.test/routes/r1");
    expect(init.method).toBe("DELETE");
    expect(init.body).toBeUndefined();
    expect((init.headers as Record<string, string>)["if-match"]).toBeUndefined();
  });

  it("retries exactly once, with a fresh token, on an unexpected 401", async () => {
    const fetcher = vi
      .fn()
      .mockResolvedValueOnce(jsonResponse(401, { code: "UNAUTHORIZED", message: "expired" }))
      .mockResolvedValueOnce(new Response(null, { status: 204 }));

    await deleteResource("/routes/r1", "deleteRoute", fetcher);

    expect(fetcher).toHaveBeenCalledTimes(2);
  });

  it("a 404 surfaces as a real GatewayError, not a silent success", async () => {
    const fetcher = vi
      .fn()
      .mockResolvedValue(jsonResponse(404, { code: "NOT_FOUND", message: "no such route" }));

    await expect(deleteResource("/routes/gone", "deleteRoute", fetcher)).rejects.toThrow(
      GatewayError,
    );
  });
});
