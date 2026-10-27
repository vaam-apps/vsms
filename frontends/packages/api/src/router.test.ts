// The router, driven the way the console drives it: through tRPC's own fetch
// adapter, on a `fetch` that never leaves the process. What is held here is
// what only the browser-side wiring can break: that the gateway data layer
// is what a procedure reaches, that a refusal keeps the tRPC code the gateway's
// status maps to, and that a validation error's per-field detail survives to
// the caller (the composer's form reads it).

import { fetchRequestHandler } from "@trpc/server/adapters/fetch";
import { configureComposeFetch, configureGatewayFetch, type GatewayFetch } from "@vsms/gateway";
import { afterEach, describe, expect, it, vi } from "vitest";
import { createContext } from "./context";
import { appRouter } from "./router";

async function call(path: string, input: unknown, method: "GET" | "POST" = "GET") {
  const encoded = encodeURIComponent(JSON.stringify({ 0: { json: input } }));
  const url =
    method === "GET"
      ? `http://page/api/trpc/${path}?batch=1&input=${encoded}`
      : `http://page/api/trpc/${path}?batch=1`;
  const response = await fetchRequestHandler({
    endpoint: "/api/trpc",
    req: new Request(url, {
      method,
      headers: { "content-type": "application/json" },
      ...(method === "POST" ? { body: JSON.stringify({ 0: { json: input } }) } : {}),
    }),
    router: appRouter,
    createContext: () => createContext({ messageStreamPollMs: 500 }),
  });
  const [result] = (await response.json()) as [
    { result?: { data: { json: unknown } }; error?: { json: { data: Record<string, unknown> } } },
  ];
  return { status: response.status, result };
}

function json(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });
}

afterEach(() => {
  configureGatewayFetch(undefined);
  configureComposeFetch(undefined);
});

describe("the router in the page", () => {
  it("reaches the gateway through the configured transport and nothing else", async () => {
    const fetcher = vi.fn<GatewayFetch>(async () => json(200, { items: [], totalCount: 0 }));
    configureGatewayFetch(fetcher);

    const { result } = await call("providers.list", undefined);

    expect(result.result).toBeDefined();
    expect(fetcher).toHaveBeenCalledTimes(1);
    const [url, init] = fetcher.mock.calls[0] as [string, RequestInit];
    expect(new URL(url).pathname).toBe("/providers");
    expect(init.method).toBe("GET");
    const names = Object.keys((init.headers ?? {}) as Record<string, string>).map((n) =>
      n.toLowerCase(),
    );
    expect(names).not.toContain("authorization");
    expect(names).not.toContain("cookie");
  });

  it("a gateway 401 is UNAUTHORIZED: the console reads it as 'not paired'", async () => {
    configureGatewayFetch(async () =>
      json(401, { code: "unauthenticated", message: "This console is not paired." }),
    );

    const { status, result } = await call("consoleKey.probe", undefined);

    expect(status).toBe(401);
    expect(result.error?.json.data.code).toBe("UNAUTHORIZED");
  });

  it("a validation error keeps its fields, so the form can say which one", async () => {
    configureComposeFetch(async () =>
      json(422, {
        code: "VALIDATION_ERROR",
        message: "invalid input",
        details: { to: ["is not a valid phone number"] },
      }),
    );

    const { status, result } = await call("compose.send", { to: "x", body: "hello" }, "POST");

    expect(status).toBe(400);
    expect(result.error?.json.data.fieldErrors).toEqual({ to: ["is not a valid phone number"] });
  });

  it("the composer is not the person's key: it goes to its own transport", async () => {
    const gateway = vi.fn<GatewayFetch>();
    configureGatewayFetch(gateway);
    const composer = vi.fn(async () => json(200, { encoding: "gsm7", segments: 1 }));
    configureComposeFetch(composer);

    const { result } = await call("compose.preview", { body: "hello" });

    expect(result.result).toBeDefined();
    expect(composer).toHaveBeenCalledWith("preview", { body: "hello" });
    expect(gateway).not.toHaveBeenCalled();
  });

  describe("a webhook endpoint the gateway would refuse never reaches it", () => {
    const base = {
      deliveryMode: "sealed",
      appId: "capp00000000000000000",
      url: "https://receiver.example/webhooks/vsms",
      audience: "https://receiver.example",
      eventTypes: ["message.delivered"],
      maskRecipient: true,
      maxAttempts: 8,
    };

    it("creates an endpoint with a sealable URL and its own audience", async () => {
      const fetcher = vi.fn<GatewayFetch>(async () =>
        json(201, { id: "cendpoint", eventTypes: " message.delivered ", version: 1 }),
      );
      configureGatewayFetch(fetcher);
      const { result } = await call("webhookEndpoints.create", base, "POST");
      expect(result.result).toBeDefined();
      expect(fetcher).toHaveBeenCalledTimes(1);
    });

    it.each([
      ["a query", "https://receiver.example/webhooks/vsms?x=1"],
      ["a bare query marker", "https://receiver.example/webhooks/vsms?"],
      ["a bare fragment marker", "https://receiver.example/webhooks/vsms#"],
      ["credentials", "https://u:pw@receiver.example/webhooks/vsms"],
      ["a path parameter", "https://receiver.example/webhooks/{tenant}"],
    ])("refuses a URL with %s, on create and on update", async (_name, url) => {
      const fetcher = vi.fn<GatewayFetch>();
      configureGatewayFetch(fetcher);
      const created = await call("webhookEndpoints.create", { ...base, url }, "POST");
      expect(created.status).toBe(400);
      const updated = await call("webhookEndpoints.update", { id: "e", etag: "1", url }, "POST");
      expect(updated.status).toBe(400);
      expect(fetcher).not.toHaveBeenCalled();
    });

    it("refuses the audience vsms, on create and on update", async () => {
      const fetcher = vi.fn<GatewayFetch>();
      configureGatewayFetch(fetcher);
      const created = await call("webhookEndpoints.create", { ...base, audience: "vsms" }, "POST");
      expect(created.status).toBe(400);
      const updated = await call(
        "webhookEndpoints.update",
        { id: "e", etag: "1", audience: " vsms " },
        "POST",
      );
      expect(updated.status).toBe(400);
      expect(fetcher).not.toHaveBeenCalled();
    });
  });

  describe("a webhook endpoint is sealed or hmac, and the router holds the two apart", () => {
    const common = {
      appId: "capp00000000000000000",
      url: "https://receiver.example/webhooks/vsms",
      eventTypes: ["message.delivered"],
      maskRecipient: true,
      maxAttempts: 8,
    };

    it("creates an hmac endpoint with no audience, sending a generated secret", async () => {
      const bodies: Record<string, unknown>[] = [];
      configureGatewayFetch(async (_url, init) => {
        bodies.push(JSON.parse(String(init?.body)));
        return json(201, {
          ...bodies[0],
          id: "cendpoint",
          eventTypes: " message.delivered ",
          version: 1,
        });
      });
      const { result } = await call(
        "webhookEndpoints.create",
        { ...common, deliveryMode: "hmac" },
        "POST",
      );
      expect(result.result).toBeDefined();
      expect(bodies[0]?.deliveryMode).toBe("hmac");
      expect(bodies[0]?.secret).toMatch(/^whsec_[0-9a-f]{64}$/);
      expect("audience" in (bodies[0] ?? {})).toBe(false);
    });

    it("refuses a sealed endpoint with no audience, and an hmac endpoint with one", async () => {
      const fetcher = vi.fn<GatewayFetch>();
      configureGatewayFetch(fetcher);
      const sealed = await call(
        "webhookEndpoints.create",
        { ...common, deliveryMode: "sealed" },
        "POST",
      );
      expect(sealed.status).toBe(400);
      const hmac = await call(
        "webhookEndpoints.create",
        { ...common, deliveryMode: "hmac", audience: "vaam" },
        "POST",
      );
      expect(hmac.status).toBe(400);
      const unknown = await call(
        "webhookEndpoints.create",
        { ...common, deliveryMode: "plain", audience: "vaam" },
        "POST",
      );
      expect(unknown.status).toBe(400);
      expect(fetcher).not.toHaveBeenCalled();
    });

    it("rotates an endpoint's secret through the procedure and nothing else", async () => {
      const requests: { url: string; body: unknown }[] = [];
      configureGatewayFetch(async (url, init) => {
        requests.push({ url: String(url), body: JSON.parse(String(init?.body)) });
        return json(200, {
          id: "cendpoint",
          deliveryMode: "hmac",
          eventTypes: " message.delivered ",
          secret: "whsec_new",
          prevSecret: "whsec_old",
          version: 2,
        });
      });
      const { result } = await call(
        "webhookEndpoints.rotateSecret",
        { endpointId: "cendpoint" },
        "POST",
      );
      expect(result.result).toBeDefined();
      expect(requests).toHaveLength(1);
      expect(requests[0]?.url).toContain("/$procs/rotateWebhookSecret");
      expect(requests[0]?.body).toEqual({ args: { endpointId: "cendpoint" } });
    });

    it("passes the server's refusal to rotate a sealed endpoint on, as the error it is", async () => {
      configureGatewayFetch(async () =>
        json(422, { code: "VALIDATION_ERROR", message: "webhook endpoint cendpoint is sealed" }),
      );
      const { status } = await call(
        "webhookEndpoints.rotateSecret",
        { endpointId: "cendpoint" },
        "POST",
      );
      expect(status).not.toBe(200);
    });
  });
});
