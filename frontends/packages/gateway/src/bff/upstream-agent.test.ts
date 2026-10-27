// The transport to the gateway, against a real local HTTP server: a sealed GET
// carries its message as the request body (which `fetch` refuses and `node:http`
// allows), a redirect is an answer and not an instruction, and an answer cannot
// be made to fill memory.

import { createServer, type IncomingMessage, type Server } from "node:http";
import type { AddressInfo } from "node:net";
import { afterEach, describe, expect, it } from "vitest";
import { MAX_RESPONSE_BYTES } from "./pass-through";
import { agentFor, createUpstream } from "./upstream-agent";

let server: Server | undefined;
afterEach(() => {
  server?.close();
  server = undefined;
});

async function listen(
  handler: (
    request: IncomingMessage,
    body: Buffer,
  ) => { status: number; headers?: Record<string, string>; body?: Buffer | string },
): Promise<string> {
  server = createServer((request, response) => {
    const chunks: Buffer[] = [];
    request.on("data", (chunk: Buffer) => chunks.push(chunk));
    request.on("end", () => {
      const reply = handler(request, Buffer.concat(chunks));
      response.writeHead(reply.status, reply.headers);
      response.end(reply.body);
    });
  });
  const listening = server;
  await new Promise<void>((resolve) => listening.listen(0, "127.0.0.1", resolve));
  return `http://127.0.0.1:${(listening.address() as AddressInfo).port}`;
}

describe("createUpstream", () => {
  it("refuses a gateway URL with a path prefix, which every request path would replace", () => {
    for (const url of ["http://gw:8080/vsms", "https://edge.example.com/api/"]) {
      expect(() => createUpstream({ url }), url).toThrow(/no path/);
    }
    expect(() => createUpstream({ url: "http://gw:8080" })).not.toThrow();
    expect(() => createUpstream({ url: "http://gw:8080/" })).not.toThrow();
  });

  it("sends a GET with the sealed message as its body, under the real method and only the given headers", async () => {
    let seen:
      | {
          method: string | undefined;
          url: string | undefined;
          headers: IncomingMessage["headers"];
          body: Buffer;
        }
      | undefined;
    const url = await listen((request, body) => {
      seen = { method: request.method, url: request.url, headers: request.headers, body };
      return { status: 200, headers: { "content-type": "application/cose" }, body: "sealed" };
    });

    const answer = await createUpstream({ url })({
      method: "GET",
      target: "/messages/m1?fields=id",
      headers: [
        ["content-type", "application/cose"],
        ["if-match", '"3"'],
      ],
      body: new Uint8Array([0xd2, 1, 2]),
    });

    expect(seen?.method).toBe("GET");
    expect(seen?.url).toBe("/messages/m1?fields=id");
    expect(Array.from(seen?.body ?? [])).toEqual([0xd2, 1, 2]);
    expect(seen?.headers["if-match"]).toBe('"3"');
    expect(seen?.headers.authorization).toBeUndefined();
    expect(seen?.headers.cookie).toBeUndefined();
    expect(answer.status).toBe(200);
    expect(new TextDecoder().decode(answer.body)).toBe("sealed");
  });

  it("returns a redirect as the answer it is, and does not follow it", async () => {
    let followed = false;
    const url = await listen((request) => {
      if (request.url === "/elsewhere") followed = true;
      return { status: 302, headers: { location: "/elsewhere" } };
    });

    const answer = await createUpstream({ url })({
      method: "POST",
      target: "/apps",
      headers: [],
      body: new Uint8Array([1]),
    });

    expect(answer.status).toBe(302);
    expect(followed).toBe(false);
  });

  it("refuses an answer over the cap instead of buffering it", async () => {
    const url = await listen(() => ({
      status: 200,
      body: Buffer.alloc(MAX_RESPONSE_BYTES + 1024),
    }));

    await expect(
      createUpstream({ url })({
        method: "GET",
        target: "/apps",
        headers: [],
        body: new Uint8Array(),
      }),
    ).rejects.toThrow();
  });

  it("times out on a gateway that never answers", async () => {
    server = createServer(() => undefined);
    await new Promise<void>((resolve) => server?.listen(0, "127.0.0.1", resolve));
    const url = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;

    await expect(
      createUpstream({ url, timeoutMs: 100 })({
        method: "GET",
        target: "/apps",
        headers: [],
        body: new Uint8Array(),
      }),
    ).rejects.toThrow();
  });

  it("will not leave the gateway's origin", async () => {
    const url = await listen(() => ({ status: 200 }));

    await expect(
      createUpstream({ url })({
        method: "GET",
        target: "//evil.example/apps",
        headers: [],
        body: new Uint8Array(),
      }),
    ).rejects.toThrow(/origin/);
  });
});

describe("agentFor", () => {
  it("wants all three certificate paths for an https gateway and none for http", () => {
    expect(() => agentFor({ url: "https://gateway:8080" })).toThrow(/certificate/);
    expect(() => agentFor({ url: "http://gateway:8080" })).not.toThrow();
  });
});
