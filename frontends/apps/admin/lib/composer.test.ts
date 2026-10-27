// The composer's route: the console's service key is spent only for a person
// the gateway has just named, in an answer that is its own, for this very
// message. The route code under test is the real `createComposer`; the gateway
// behind it is a real sealed verifier that decides `authorizeComposition` the
// way the real one does; the browser side is the real `createComposeFetch`
// signing with a real WebCrypto key; the answer is opened by the real opener
// (cratestack's envelope, through the same wasm module the SDK loads).

import { randomBytes } from "node:crypto";
import {
  type ComposeOperation,
  type CompositionAuthorization,
  composeProofBody,
  createComposeFetch,
  type SealedProof,
  sha256Hex,
} from "@vsms/gateway";
import { type ComposerClient, createComposer } from "@vsms/gateway/bff";
import { describe, expect, it, vi } from "vitest";
import { base64Url } from "./pairing";
import {
  authorizeComposition,
  composeOpener,
  composeUpstream,
  type FakeGateway,
  type Principal,
  type Signer,
  startFakeGateway,
} from "./test-support/fake-gateway";

const ADMIN: Principal = { kind: "user", id: "u1", role: "admin", perms: ["sms:send", "sms:read"] };

function clientSpy(): ComposerClient & {
  previewMessage: ReturnType<typeof vi.fn>;
  sendMessage: ReturnType<typeof vi.fn>;
} {
  return {
    previewMessage: vi.fn(async () => ({ encoding: "gsm7", segments: 1 })),
    sendMessage: vi.fn(async () => ({ result: { messageId: "m1", state: "queued" } })),
  };
}

/** The browser's key stands as `principal` at the gateway; `extra` are other
 *  keys the gateway knows (an app client's, say). No principal: unpaired. */
async function harness(options: { browser?: Principal | null; extra?: Signer[] } = {}) {
  const gateway = await startFakeGateway();
  const browser = options.browser === undefined ? ADMIN : options.browser;
  const signers: Signer[] = [
    ...(browser === null
      ? []
      : [{ publicKey: base64Url(gateway.key.publicKey), principal: browser }]),
    ...(options.extra ?? []),
  ];
  const upstream = composeUpstream(gateway, signers);
  const client = clientSpy();
  const composer = createComposer({
    upstream,
    client: () => client,
    opener: () => composeOpener(gateway),
  });
  const unauthenticated = vi.fn();
  const send = (async (url: string, init: RequestInit) =>
    composer(new Request(`http://console.test${url}`, init))) as unknown as typeof fetch;
  const compose = createComposeFetch({
    runtime: async () => gateway.runtime,
    endpoint: "/api/compose",
    send,
    onUnauthenticated: unauthenticated,
  });
  return { gateway, client, upstream, composer, compose, unauthenticated };
}

const MESSAGE = { to: "+237677123456", body: "hello" };

/** A proof sealed by `sealer` for `operation` and the hash of `inputText`. */
async function proofFrom(
  gateway: FakeGateway,
  sealer: {
    sealCall: FakeGateway["runtime"]["sealer"]["sealCall"];
  },
  operation: string,
  inputText: string,
  body?: string,
): Promise<SealedProof> {
  const { route, params } = gateway.runtime.routeOf("POST", "/$procs/authorizeComposition");
  const sealed = await sealer.sealCall(
    "POST",
    route,
    params,
    body ?? composeProofBody(operation as ComposeOperation, await sha256Hex(inputText)),
    undefined,
    undefined,
    undefined,
  );
  return { headers: sealed.headers, body: Buffer.from(sealed.body).toString("base64") };
}

function post(composer: (r: Request) => Promise<Response>, operation: string, body: unknown) {
  return composer(
    new Request(`http://console.test/api/compose/${operation}`, {
      method: "POST",
      body: JSON.stringify(body),
    }),
  );
}

/** An app client's Ed25519 key, registered at the gateway with `scopes`. */
function appClient(gateway: FakeGateway, scopes: string[]): { signer: Signer; sealer: never } {
  const seed = randomBytes(32).toString("base64url");
  const sealer = gateway.module.Sealer.fromSeed(
    seed,
    gateway.config.serverKeys.current,
    gateway.config.serverKeys.next,
  );
  return {
    signer: {
      publicKey: sealer.publicKey,
      principal: { kind: "app", id: "appc_other", role: "app", perms: scopes },
    },
    sealer: sealer as never,
  };
}

describe("the composer spends the service key only for a person the gateway names", () => {
  it("previews and sends for a paired person who may send", async () => {
    const h = await harness();

    const preview = await h.compose("preview", MESSAGE);
    const send = await h.compose("send", MESSAGE);

    expect(preview.status).toBe(200);
    expect(await preview.json()).toEqual({ encoding: "gsm7", segments: 1 });
    expect(send.status).toBe(200);
    expect(await send.json()).toEqual({ messageId: "m1", state: "queued" });
    expect(h.client.previewMessage).toHaveBeenCalledWith(MESSAGE);
    expect(h.client.sendMessage).toHaveBeenCalledWith(MESSAGE);
    // The proof the route forwarded was one call to the procedure, each time.
    expect(h.upstream.seen).toEqual([
      "/$procs/authorizeComposition",
      "/$procs/authorizeComposition",
    ]);
  });

  it("does not use it for an app client's key, whatever scopes that client holds", async () => {
    // The gateway answers 200 to a sealed read from any signer it accepts. The
    // procedure it answers only for a person, so an app client's proof is a 403.
    const gateway = await startFakeGateway();
    const app = appClient(gateway, ["sms:send", "sms:read"]);
    const upstream = composeUpstream(gateway, [app.signer]);
    const client = clientSpy();
    const composer = createComposer({
      upstream,
      client: () => client,
      opener: () => composeOpener(gateway),
    });
    const inputText = JSON.stringify(MESSAGE);
    const proof = await proofFrom(gateway, app.sealer, "send", inputText);

    const response = await post(composer, "send", { proof, input: inputText });

    expect(response.status).toBe(403);
    expect(((await response.json()) as { code: string }).code).toBe("forbidden");
    expect(client.sendMessage).not.toHaveBeenCalled();
    expect(client.previewMessage).not.toHaveBeenCalled();
  });

  it("does not use it for a paired person who may not send, nor for one whose role lost the permission", async () => {
    for (const principal of [
      { ...ADMIN, role: "auditor", perms: ["sms:read"] },
      { ...ADMIN, role: "support", perms: ["sms:read"] },
      { ...ADMIN, role: "developer", perms: ["sms:send", "sms:read"] },
      { ...ADMIN, role: "operator", perms: ["sms:read"] },
    ]) {
      const h = await harness({ browser: principal });

      const response = await h.compose("send", MESSAGE);

      expect(response.status, principal.role).toBe(403);
      expect(h.client.sendMessage, principal.role).not.toHaveBeenCalled();
    }
  });

  it("does not use it for a browser the gateway does not know, and says 'not paired'", async () => {
    const h = await harness({ browser: null });

    const response = await h.compose("send", MESSAGE);

    expect(response.status).toBe(401);
    expect(((await response.json()) as { code: string }).code).toBe("unauthenticated");
    expect(h.client.sendMessage).not.toHaveBeenCalled();
    expect(h.unauthenticated).toHaveBeenCalled();
  });

  it("does not send a different message than the one the proof was made for", async () => {
    const h = await harness();
    const proved = JSON.stringify(MESSAGE);
    const proof = await proofFrom(h.gateway, h.gateway.runtime.sealer, "send", proved);

    const other = JSON.stringify({ to: "+237677999999", body: "pay me" });
    const response = await post(h.composer, "send", { proof, input: other });

    expect(response.status).toBe(403);
    expect(h.client.sendMessage).not.toHaveBeenCalled();
  });

  it("does not send what it was only allowed to preview", async () => {
    const h = await harness();
    const inputText = JSON.stringify(MESSAGE);
    const proof = await proofFrom(h.gateway, h.gateway.runtime.sealer, "preview", inputText);

    const response = await post(h.composer, "send", { proof, input: inputText });

    expect(response.status).toBe(403);
    expect(h.client.sendMessage).not.toHaveBeenCalled();
  });

  it("sends exactly the text that was proved, however it would parse", async () => {
    const h = await harness();
    // Duplicate keys and odd spacing: the hash is of these bytes, not of a
    // value written out again.
    const text = '{ "to":"+237677123456", "body":"a", "body":"b" }';
    const proof = await proofFrom(h.gateway, h.gateway.runtime.sealer, "send", text);

    const response = await post(h.composer, "send", { proof, input: text });

    expect(response.status).toBe(200);
    expect(h.client.sendMessage).toHaveBeenCalledWith({ to: "+237677123456", body: "b" });
    // The same bytes with a space moved are another message.
    const moved = await post(h.composer, "send", {
      proof: await proofFrom(h.gateway, h.gateway.runtime.sealer, "send", text),
      input: text.replace('"to":', '"to": '),
    });
    expect(moved.status).toBe(403);
  });

  it("does not use it for a proof that is replayed: a proof is single-use", async () => {
    const h = await harness();
    const inputText = JSON.stringify(MESSAGE);
    const proof = await proofFrom(h.gateway, h.gateway.runtime.sealer, "send", inputText);

    const first = await post(h.composer, "send", { proof, input: inputText });
    const replay = await post(h.composer, "send", { proof, input: inputText });

    expect(first.status).toBe(200);
    expect(replay.status).toBe(401);
    expect(h.client.sendMessage).toHaveBeenCalledTimes(1);
  });

  it("does not use it for a proof of some other call: the route is bound into the signature", async () => {
    const h = await harness();
    const inputText = JSON.stringify(MESSAGE);
    const { route, params } = h.gateway.runtime.routeOf("GET", "/apps");
    const sealed = await h.gateway.runtime.sealer.sealCall(
      "GET",
      route,
      params,
      undefined,
      "limit=1",
      undefined,
      undefined,
    );
    const proof = { headers: sealed.headers, body: Buffer.from(sealed.body).toString("base64") };

    const response = await post(h.composer, "send", { proof, input: inputText });

    expect(response.status).toBe(401);
    expect(h.client.sendMessage).not.toHaveBeenCalled();
  });

  it("does not believe an answer that is not the gateway's, even one that says 200 and names a person", async () => {
    const h = await harness();
    const inputText = JSON.stringify(MESSAGE);
    const proof = await proofFrom(h.gateway, h.gateway.runtime.sealer, "send", inputText);
    const hash = await sha256Hex(inputText);
    const claim: CompositionAuthorization = {
      userId: "u1",
      role: "admin",
      operation: "send",
      inputSha256: hash,
    };
    const impostor = createComposer({
      upstream: async () => ({
        status: 200,
        headers: new Headers({ "content-type": "application/json" }),
        body: new TextEncoder().encode(JSON.stringify(claim)),
      }),
      client: () => h.client,
      opener: () => composeOpener(h.gateway),
    });

    const response = await post(impostor, "send", { proof, input: inputText });

    expect(response.status).toBe(502);
    expect(h.client.sendMessage).not.toHaveBeenCalled();
  });

  it("does not use it when the proof is missing, malformed or has no input", async () => {
    const h = await harness();
    const inputText = JSON.stringify(MESSAGE);
    const proof = await proofFrom(h.gateway, h.gateway.runtime.sealer, "send", inputText);

    for (const body of [
      {},
      { input: inputText },
      { proof: { headers: "no", body: "AAAA" }, input: inputText },
      { proof: { headers: [], body: "" }, input: inputText },
      { proof, input: "not json" },
      { proof, input: '"a string, not an object"' },
      { proof, input: "[1]" },
      { proof, input: { to: "x" } },
      { proof, input: "" },
      // A lone surrogate: UTF-8 would hash it as U+FFFD, the same as a real one.
      { proof, input: '{"to":"\ud800"}' },
    ]) {
      expect((await post(h.composer, "send", body)).status).toBe(400);
    }
    expect(h.client.sendMessage).not.toHaveBeenCalled();
  });

  it("answers 400, not 503, for a proof with a header that is not a header", async () => {
    const h = await harness();
    const inputText = JSON.stringify(MESSAGE);
    const proof = await proofFrom(h.gateway, h.gateway.runtime.sealer, "send", inputText);

    for (const header of [
      ["not a header name", "x"],
      ["content-type\r\nx-injected", "x"],
      ["x-ok", "two\r\nlines"],
      ["", "x"],
      ["x-ok", "beyond latin-1: \u4e2d"],
    ] as [string, string][]) {
      const response = await post(h.composer, "send", {
        proof: { ...proof, headers: [...proof.headers, header] },
        input: inputText,
      });
      expect(response.status, JSON.stringify(header)).toBe(400);
    }
    expect(h.upstream.seen).toEqual([]);
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
      opener: () => composeOpener(gateway),
    });
    const inputText = JSON.stringify(MESSAGE);
    const proof = await proofFrom(gateway, gateway.runtime.sealer, "send", inputText);

    const response = await post(composer, "send", { proof, input: inputText });

    expect(response.status).toBe(503);
    expect(client.sendMessage).not.toHaveBeenCalled();
  });

  it("refuses any operation but preview and send, and any method but POST", async () => {
    const h = await harness();
    const inputText = JSON.stringify(MESSAGE);
    const proof = await proofFrom(h.gateway, h.gateway.runtime.sealer, "send", inputText);

    expect((await post(h.composer, "delete-everything", { proof, input: inputText })).status).toBe(
      404,
    );
    const get = await h.composer(new Request("http://console.test/api/compose/send"));
    expect(get.status).toBe(405);
  });
});

describe("the fake gateway decides authorizeComposition as the real one does", () => {
  const body = (operation: string, hash: string) =>
    JSON.stringify({ args: { operation, inputSha256: hash } });
  const hash = "a".repeat(64);

  it("admits a person of an admitted role holding sms:send, and names them", () => {
    const reply = authorizeComposition(ADMIN, body("send", hash));
    expect(reply.status).toBe(200);
    expect(JSON.parse(reply.body)).toEqual({
      userId: "u1",
      role: "admin",
      operation: "send",
      inputSha256: hash,
    });
  });

  it("refuses an app, a role Layer 1 does not admit, and a role without the permission", () => {
    expect(
      authorizeComposition({ ...ADMIN, kind: "app", role: "app" }, body("send", hash)).status,
    ).toBe(403);
    expect(authorizeComposition({ ...ADMIN, role: "auditor" }, body("send", hash)).status).toBe(
      403,
    );
    expect(authorizeComposition({ ...ADMIN, perms: [] }, body("send", hash)).status).toBe(403);
  });

  it("refuses an unknown operation and a malformed hash", () => {
    expect(authorizeComposition(ADMIN, body("delete", hash)).status).toBe(422);
    expect(authorizeComposition(ADMIN, body("send", "xyz")).status).toBe(422);
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
