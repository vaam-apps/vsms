import { strict as assert } from "node:assert";
import { test } from "node:test";
import { parseBaseUrl, sealedUrl } from "../dist/base-url.js";
import { SdkError, VsmsClient } from "../dist/index.js";
import { INPUT, ok, rejection, SEND_RESULT, withServer } from "../support/helpers.ts";
import { newTestIdentity } from "../support/responder.ts";
import { startTestServer } from "../support/server.ts";

// The signature binds a route and its parameters; the URL is what reaches the
// gateway. These tests hold the two to each other: whatever is wrong with the
// address or a parameter, the request is refused here, before it is sent,
// rather than signed for one place and delivered to another.

function configError(thunk: () => unknown): SdkError {
  try {
    thunk();
  } catch (err) {
    assert.ok(err instanceof SdkError, String(err));
    assert.equal(err.kind, "config", err.message);
    return err;
  }
  assert.fail("expected a config error");
}

function clientAt(baseUrl: string): VsmsClient {
  const identity = newTestIdentity();
  return new VsmsClient({
    baseUrl,
    privateKey: identity.clientSeed,
    serverKeys: { current: identity.serverPublic, next: identity.nextPublic },
  });
}

// ----------------------------------------------------------- the base URL

test("a base URL with a fragment, a query or credentials is refused at construction", () => {
  for (const baseUrl of [
    "http://h/#",
    "http://h#",
    "http://h/#frag",
    "http://h/?",
    "http://h?",
    "http://h/?x=1",
    "http://h/gw?x=1",
    "http://user:secret@h",
    "http://user@h/",
    "http://:secret@h",
    "https://user:secret@h/gw",
  ]) {
    const error = configError(() => clientAt(baseUrl));
    assert.equal(error.message.includes("secret"), false, `${baseUrl} was quoted`);
  }
});

test("a base URL that is not http or https, or not a URL, is refused", () => {
  for (const baseUrl of [
    "ftp://h",
    "file:///tmp",
    "ws://h",
    "h:80",
    "//h",
    "/gw",
    "not a url",
    " ",
  ]) {
    configError(() => clientAt(baseUrl));
  }
  for (const baseUrl of [undefined, null, 5, {}] as unknown[]) {
    configError(() => clientAt(baseUrl as string));
  }
});

test("a gateway under a path prefix is addressed through it, whatever trailing slashes it was given", () => {
  assert.deepEqual(parseBaseUrl("http://h"), { origin: "http://h", basePath: "" });
  assert.deepEqual(parseBaseUrl("http://h/"), { origin: "http://h", basePath: "" });
  assert.deepEqual(parseBaseUrl("http://h///"), { origin: "http://h", basePath: "" });
  assert.deepEqual(parseBaseUrl("https://h:8443/api/v1//"), {
    origin: "https://h:8443",
    basePath: "/api/v1",
  });
  const huge = `http://h/${"/".repeat(200_000)}`;
  assert.equal(parseBaseUrl(huge).basePath, "", "a long run of slashes is handled in linear time");
});

test("the request goes to the prefix and the signed target, and carries no Authorization header", async () => {
  const gateway = await startTestServer((_request, response) => {
    response.writeHead(401).end();
  });
  try {
    const client = clientAt(`${gateway.url}/gw//`);
    await rejection(client.sendMessage(INPUT));
    assert.equal(gateway.requests[0]?.url, "/gw/$procs/sendMessage");
    assert.equal(gateway.requests[0]?.headers.authorization, undefined);
  } finally {
    await gateway.close();
  }
});

test("userinfo in the base URL never reaches the wire: no Authorization header, no request", async () => {
  const gateway = await startTestServer((_request, response) => {
    response.writeHead(401).end();
  });
  try {
    const withUserinfo = gateway.url.replace("http://", "http://user:secret@");
    configError(() => clientAt(withUserinfo));
    assert.equal(gateway.requests.length, 0);
  } finally {
    await gateway.close();
  }
});

// ----------------------------------------------------------- the parameters

test("a dot segment as an id is refused before it is signed or sent", async () => {
  await withServer(
    () => ok({ id: "x" }),
    async (server, client) => {
      for (const id of [".", ".."]) {
        const error = await rejection(client.getMessage(id));
        assert.equal(error.kind, "seal", id);
        assert.match(error.message, /dot segment/);
      }
      assert.equal(server.requests.length, 0, "nothing was sent");
    },
  );
});

test("an id that merely looks like a dot segment is text, signed and sent as such", async () => {
  await withServer(
    () => ok({ id: "x" }),
    async (server, client) => {
      for (const [id, onTheWire] of [
        ["%2e%2e", "%252e%252e"],
        ["..x", "..x"],
        ["a.b", "a.b"],
        ["a/../b", "a%2F..%2Fb"],
      ] as const) {
        await client.getMessage(id);
        const seen = server.opened.at(-1);
        assert.equal(seen?.url, `/messages/${onTheWire}`, id);
        assert.deepEqual(seen?.params, [id], `the signature binds ${id}`);
      }
    },
  );
});

// --------------------------------------------- the URL, checked against the target

test("a target that URL parsing would rewrite, resolve or cut short is not sent", () => {
  const base = parseBaseUrl("http://127.0.0.1:8080/gw");
  assert.equal(sealedUrl(base, "/messages/m1"), "http://127.0.0.1:8080/gw/messages/m1");
  assert.equal(
    sealedUrl(base, "/messages/m1?fields=id"),
    "http://127.0.0.1:8080/gw/messages/m1?fields=id",
  );
  for (const target of [
    "/messages/..", // resolved to /gw/
    "/messages/%2e%2e", // so is its escaped spelling
    "/messages/./m1",
    "/a/../b",
    "/messages/m1#frag", // cut short
    "/messages/m1#",
    "/messages/m1?a=b c", // rewritten
    "/messages\\m1", // a backslash is a slash to a URL parser
    "/messages/m1\n",
  ]) {
    assert.throws(
      () => sealedUrl(base, target),
      (err: unknown) => err instanceof SdkError && err.kind === "seal",
      JSON.stringify(target),
    );
  }
});

test("the URL is built from the parsed base and not from the caller's text", () => {
  // `h` and `H` and a default port are the same origin; the URL says so once.
  const base = parseBaseUrl("HTTP://EXAMPLE.com:80/gw/");
  assert.equal(sealedUrl(base, "/messages/m1"), "http://example.com/gw/messages/m1");
});

test("a signed call still reaches its route and parameters unchanged through a prefix-free base", async () => {
  await withServer(
    () => ok(SEND_RESULT),
    async (server, client) => {
      await client.getMessage("a/b c");
      assert.equal(server.requests[0]?.url, "/messages/a%2Fb%20c");
    },
  );
});
