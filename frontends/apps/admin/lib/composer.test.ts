// The composer's route: the console's service key is spent only for a browser
// the gateway has just vouched for. The route code under test is the real
// `createComposer`; the gateway behind it is a real sealed verifier; the browser
// side is the real `createComposeFetch` signing with a real WebCrypto key.

import { createComposeFetch, type SealedProof } from "@vsms/gateway";
import { type ComposerClient, createComposer, type Upstream } from "@vsms/gateway/bff";
import { describe, expect, it, vi } from "vitest";
import { type FakeGateway, startFakeGateway } from "./test-support/fake-gateway";

/** The route's upstream, standing in for the real mTLS call: the gateway's
 *  verifier decides, exactly as the real gateway does, and says 200 or the
 *  coarse unsigned 401. */
function upstreamOf(gateway: FakeGateway, seen: { target: string }[] = []): Upstream {
  return async ({ method, target, headers, body }) => {
    seen.push({ target });
    const [path = "", query] = target.split("?");
    try {
      const opened = await gateway.responder.openRequest(method, path, query, headers, body);
      const sealed = await opened.sealResponse(200, '{"items":[]}');
      return { status: 200, headers: new Headers(sealed.headers), body: sealed.body };
    } catch {
      return { status: 401, headers: new Headers(), body: new TextEncoder().encode("unsigned") };
    }
  };
}

function clientSpy(): ComposerClient & {
  previewMessage: ReturnType<typeof vi.fn>;
  sendMessage: ReturnType<typeof vi.fn>;
} {
  return {
    previewMessage: vi.fn(async () => ({ encoding: "gsm7", segments: 1 })),
    sendMessage: vi.fn(async () => ({ result: { messageId: "m1", state: "queued" } })),
  };
}

async function harness(options: { registered?: boolean } = {}) {
  const gateway = await startFakeGateway(options);
  const client = clientSpy();
  const seen: { target: string }[] = [];
  const composer = createComposer({ upstream: upstreamOf(gateway, seen), client: () => client });
  const unauthenticated = vi.fn();
  const send = (async (url: string, init: RequestInit) =>
    composer(new Request(`http://console.test${url}`, init))) as unknown as typeof fetch;
  const compose = createComposeFetch({
    runtime: async () => gateway.runtime,
    endpoint: "/api/compose",
    send,
    onUnauthenticated: unauthenticated,
  });
  return { gateway, client, seen, composer, compose, unauthenticated };
}

async function proofFor(gateway: FakeGateway, path: string, query?: string): Promise<SealedProof> {
  const { route, params } = gateway.runtime.routeOf("GET", path);
  const sealed = await gateway.runtime.sealer.sealCall(
    "GET",
    route,
    params,
    undefined,
    query,
    undefined,
    undefined,
  );
  return {
    headers: sealed.headers,
    body: Buffer.from(sealed.body).toString("base64"),
  };
}

function post(composer: (r: Request) => Promise<Response>, operation: string, body: unknown) {
  return composer(
    new Request(`http://console.test/api/compose/${operation}`, {
      method: "POST",
      body: JSON.stringify(body),
    }),
  );
}

describe("the composer spends the service key only for a paired browser", () => {
  it("previews and sends for a browser whose key the gateway accepts", async () => {
    const h = await harness();

    const preview = await h.compose("preview", { body: "hello", to: "+237677123456" });
    const send = await h.compose("send", { to: "+237677123456", body: "hello" });

    expect(preview.status).toBe(200);
    expect(await preview.json()).toEqual({ encoding: "gsm7", segments: 1 });
    expect(send.status).toBe(200);
    expect(await send.json()).toEqual({ messageId: "m1", state: "queued" });
    expect(h.client.previewMessage).toHaveBeenCalledWith({ body: "hello", to: "+237677123456" });
    expect(h.client.sendMessage).toHaveBeenCalledTimes(1);
    // The proof the route forwarded was the one cheap read, to the letter.
    expect(h.seen.map((s) => s.target)).toEqual([
      "/sender_ids?fields=id&limit=1",
      "/sender_ids?fields=id&limit=1",
    ]);
  });

  it("does not use the service key for a browser the gateway does not know, and says 'not paired'", async () => {
    const h = await harness({ registered: false });

    const response = await h.compose("send", { to: "+237677123456", body: "hello" });

    expect(response.status).toBe(401);
    expect(((await response.json()) as { code: string }).code).toBe("unauthenticated");
    expect(h.client.sendMessage).not.toHaveBeenCalled();
    expect(h.unauthenticated).toHaveBeenCalled();
  });

  it("does not use it for a proof that is replayed: a proof is single-use", async () => {
    const h = await harness();
    const proof = await proofFor(h.gateway, "/sender_ids", "fields=id&limit=1");

    const first = await post(h.composer, "send", { proof, input: { to: "x", body: "y" } });
    const replay = await post(h.composer, "send", { proof, input: { to: "x", body: "y" } });

    expect(first.status).toBe(200);
    expect(replay.status).toBe(401);
    expect(h.client.sendMessage).toHaveBeenCalledTimes(1);
  });

  it("does not use it for a proof of some other read: the route is bound into the signature", async () => {
    const h = await harness();
    const proof = await proofFor(h.gateway, "/apps", "limit=1");

    const response = await post(h.composer, "send", { proof, input: { to: "x", body: "y" } });

    expect(response.status).toBe(401);
    expect(h.client.sendMessage).not.toHaveBeenCalled();
  });

  it("does not use it when the proof is missing, malformed or has no input", async () => {
    const h = await harness();
    const proof = await proofFor(h.gateway, "/sender_ids", "fields=id&limit=1");

    for (const body of [
      {},
      { input: { to: "x" } },
      { proof: { headers: "no", body: "AAAA" }, input: {} },
      { proof: { headers: [], body: "" }, input: {} },
      { proof, input: "not an object" },
    ]) {
      expect((await post(h.composer, "send", body)).status).toBe(400);
    }
    expect(h.client.sendMessage).not.toHaveBeenCalled();
  });

  it("says the gateway is unreachable when it is, and still does not send", async () => {
    const gateway = await startFakeGateway();
    const client = clientSpy();
    const composer = createComposer({
      upstream: async () => {
        throw new Error("ECONNREFUSED");
      },
      client: () => client,
    });
    const proof = await proofFor(gateway, "/sender_ids", "fields=id&limit=1");

    const response = await post(composer, "send", { proof, input: { to: "x", body: "y" } });

    expect(response.status).toBe(503);
    expect(client.sendMessage).not.toHaveBeenCalled();
  });

  it("refuses any operation but preview and send, and any method but POST", async () => {
    const h = await harness();
    const proof = await proofFor(h.gateway, "/sender_ids", "fields=id&limit=1");

    expect((await post(h.composer, "delete-everything", { proof, input: {} })).status).toBe(404);
    const get = await h.composer(new Request("http://console.test/api/compose/send"));
    expect(get.status).toBe(405);
  });
});

describe("what the service key's client says is passed on as the gateway said it", () => {
  const sdkError = (fields: Record<string, unknown>, message: string) =>
    Object.assign(new Error(message), { name: "SdkError", ...fields });

  it("keeps a validation error's status, code and per-field detail for the form", async () => {
    const h = await harness();
    h.client.sendMessage.mockRejectedValueOnce(
      sdkError(
        {
          kind: "remote",
          httpStatus: 422,
          gatewayCode: "VALIDATION_ERROR",
          fieldErrors: { to: ["is not a valid phone number"] },
        },
        "invalid input",
      ),
    );

    const response = await h.compose("send", { to: "x", body: "y" });

    expect(response.status).toBe(422);
    expect(await response.json()).toEqual({
      code: "VALIDATION_ERROR",
      message: "invalid input",
      details: { to: ["is not a valid phone number"] },
    });
  });

  it("does not leak why the service key's own call failed", async () => {
    const h = await harness();
    h.client.sendMessage.mockRejectedValueOnce(
      sdkError({ kind: "unverified" }, "the answer does not verify under a pinned server key"),
    );

    const response = await h.compose("send", { to: "x", body: "y" });
    const body = (await response.json()) as { code: string; message: string };

    expect(response.status).toBe(502);
    expect(body.code).toBe("composer_unavailable");
    expect(body.message).not.toContain("pinned");
  });
});
