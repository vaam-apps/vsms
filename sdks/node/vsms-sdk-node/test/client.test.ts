import { strict as assert } from "node:assert";
import { createPrivateKey, createPublicKey, generateKeyPairSync, sign } from "node:crypto";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:http";
import type { AddressInfo } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { parseBaseUrl } from "../dist/base-url.js";
import { MAX_RESPONSE_BYTES, SdkError, VsmsClient } from "../dist/index.js";
import { performSealedCall } from "../dist/sealed-call.js";
import type { WasmSealer } from "../dist/wasm.js";
import { INPUT, ok, rejection, SEND_RESULT, withServer } from "../support/helpers.ts";
import { newTestIdentity } from "../support/responder.ts";
import { startSealedServer, startTestServer } from "../support/server.ts";

// ------------------------------------------------------------ what is sent

test("sendMessage seals the call: the gateway reads back the route, the JSON and the idempotency key", async () => {
  await withServer(
    () => ok(SEND_RESULT),
    async (server, client) => {
      const outcome = await client.sendMessage(INPUT, { idempotencyKey: "retry-1" });
      assert.deepEqual(outcome.result, SEND_RESULT);
      assert.equal(outcome.idempotencyReplayed, false);

      const [seen] = server.opened;
      assert.equal(seen?.method, "POST");
      assert.equal(seen?.route, "/$procs/sendMessage");
      assert.deepEqual(seen?.params, []);
      assert.deepEqual(JSON.parse(seen?.payload ?? ""), { args: INPUT });
      assert.equal(seen?.idempotencyKey, "retry-1");
    },
  );
});

test("a sealed request carries no bearer, no cookie and no plain body", async () => {
  await withServer(
    () => ok(SEND_RESULT),
    async (server, client) => {
      await client.sendMessage(INPUT);
      const [wire] = server.requests;
      assert.ok(wire);
      assert.equal(wire.headers.authorization, undefined);
      assert.equal(wire.headers.cookie, undefined);
      assert.match(String(wire.headers["content-type"]), /^application\/cose/);
      // The body is the COSE message, not the JSON the caller handed over.
      assert.notEqual(wire.body.toString("utf8"), JSON.stringify({ args: INPUT }));
      assert.equal(wire.headers["cratestack-payload-type"], "application/json");
      assert.ok(wire.headers["cratestack-contract"]);
    },
  );
});

test("a sealed GET carries the signed message as its body", async () => {
  await withServer(
    () => ok({ id: "m1" }),
    async (server, client) => {
      await client.getMessage("m1");
      const [wire] = server.requests;
      assert.equal(wire?.method, "GET");
      assert.ok((wire?.body.length ?? 0) > 0, "a bodiless GET still has to carry its signature");
      assert.equal(wire?.headers["content-length"], String(wire?.body.length));
    },
  );
});

test("previewMessage seals its own operation", async () => {
  const preview = {
    encoding: "gsm7",
    segments: 1,
    length: 4,
    perSegment: 160,
    offending: [],
    operator: "orange",
  };
  await withServer(
    () => ok(preview),
    async (server, client) => {
      assert.deepEqual(await client.previewMessage({ body: "test" }), preview);
      assert.equal(server.opened[0]?.route, "/$procs/previewMessage");
    },
  );
});

test("getMessage binds the id the URL carries, whatever characters it holds", async () => {
  await withServer(
    () => ok({ id: "a/b c" }),
    async (server, client) => {
      await client.getMessage("a/b c");
      assert.equal(server.opened[0]?.route, "/messages/{id}");
      assert.deepEqual(server.opened[0]?.params, ["a/b c"]);
      assert.equal(server.requests[0]?.url, "/messages/a%2Fb%20c");
    },
  );
});

test("the replay hint is read from the response header, and is only a hint", async () => {
  await withServer(
    () => ok(SEND_RESULT, { "idempotency-replayed": "true" }),
    async (_server, client) => {
      const outcome = await client.sendMessage(INPUT, { idempotencyKey: "k" });
      assert.equal(outcome.idempotencyReplayed, true);
    },
  );
});

test("the replay hint is read case-insensitively, as the Rust SDK reads it", async () => {
  for (const value of ["True", "TRUE", "true"]) {
    await withServer(
      () => ok(SEND_RESULT, { "idempotency-replayed": value }),
      async (_server, client) => {
        assert.equal((await client.sendMessage(INPUT)).idempotencyReplayed, true, value);
      },
    );
  }
  await withServer(
    () => ok(SEND_RESULT, { "idempotency-replayed": "false" }),
    async (_server, client) => {
      assert.equal((await client.sendMessage(INPUT)).idempotencyReplayed, false);
    },
  );
});

// ------------------------------------------------------ what is believed

test("a sealed error answer is the gateway's, with its status and code", async () => {
  await withServer(
    () => ({
      status: 422,
      body: JSON.stringify({
        code: "VALIDATION_ERROR",
        message: "to is not a number",
        details: { to: ["invalid"] },
      }),
    }),
    async (_server, client) => {
      const error = await rejection(client.sendMessage(INPUT));
      assert.equal(error.kind, "remote");
      assert.equal(error.httpStatus, 422);
      assert.equal(error.gatewayCode, "VALIDATION_ERROR");
      assert.equal(error.message, "to is not a number");
      assert.deepEqual(error.fieldErrors, { to: ["invalid"] });
    },
  );
});

test("a sealed non-JSON error body becomes the message", async () => {
  await withServer(
    () => ({
      status: 409,
      body: "another request with this Idempotency-Key is still in flight",
    }),
    async (_server, client) => {
      const error = await rejection(client.sendMessage(INPUT, { idempotencyKey: "k" }));
      assert.equal(error.isConflict(), true);
      assert.equal(error.isIdempotencyInFlight(), true);
    },
  );
});

test("a sealed 2xx whose body is not JSON is a decode error, not a result", async () => {
  await withServer(
    () => ({ status: 200, body: "<html>" }),
    async (_server, client) => {
      const error = await rejection(client.sendMessage(INPUT));
      assert.equal(error.kind, "decode");
      assert.equal(error.httpStatus, 200);
    },
  );
});

test("an answer that arrives and cannot be read is unreadable, never seal: the request was sent", async () => {
  await withServer(
    () => ({
      status: 200,
      body: JSON.stringify(SEND_RESULT),
      // Sealed under the right key for this very request, and delivered with a
      // payload type this client did not ask for.
      tamper: (sealed) =>
        Object.fromEntries(
          Object.entries(sealed).map(([name, value]) => [
            name,
            name.toLowerCase() === "cratestack-payload-type" ? "text/plain" : value,
          ]),
        ),
    }),
    async (server, client) => {
      const error = await rejection(client.sendMessage(INPUT, { idempotencyKey: "k-1" }));
      assert.equal(error.kind, "unreadable");
      assert.equal(error.httpStatus, 200);
      assert.equal(
        server.opened.length,
        1,
        "the request was delivered, so this is not a seal failure",
      );
      assert.equal(server.opened[0]?.idempotencyKey, "k-1");
    },
  );
});

test("anything that fails in opening the answer, even a trap in the module, is unreadable and not seal", async () => {
  const gateway = await startTestServer((_request, response) => {
    response.writeHead(200, { "content-type": "application/json" }).end("{}");
  });
  try {
    for (const thrown of [
      new TypeError("the module trapped"),
      { kind: "invalid", message: "headers must be an array of [name, value] string pairs" },
      { kind: "unreadable", message: "the answer is not UTF-8" },
    ]) {
      // A sealer that signs nothing: the call goes out as a plain request, and
      // what is under test is only how a failure in `openResponse` is named.
      const sealer = {
        publicKey: "",
        algorithm: "ed25519",
        sealCall: async () => ({
          body: new Uint8Array([1]),
          headers: [],
          target: "/$procs/sendMessage",
          openResponse: async () => {
            throw thrown;
          },
        }),
      } as unknown as WasmSealer;
      const error = await rejection(
        performSealedCall(
          { base: parseBaseUrl(gateway.url), timeoutMs: 2_000, sealer },
          { method: "POST", route: "/$procs/sendMessage" },
        ),
      );
      assert.equal(error.kind, "unreadable", JSON.stringify(thrown));
      assert.equal(error.httpStatus, 200);
    }
    assert.equal(gateway.requests.length, 3, "every one of them had been delivered");
  } finally {
    await gateway.close();
  }
});

test("a plain 200 is not believed, whatever it says", async () => {
  const plain = await startTestServer((_request, response) => {
    response.writeHead(200, { "content-type": "application/json" });
    response.end(JSON.stringify(SEND_RESULT));
  });
  try {
    const identity = newTestIdentity();
    const client = new VsmsClient({
      baseUrl: plain.url,
      privateKey: identity.clientSeed,
      serverKeys: { current: identity.serverPublic, next: identity.nextPublic },
    });
    const error = await rejection(client.sendMessage(INPUT));
    assert.equal(error.kind, "unsigned");
    assert.equal(error.httpStatus, 200);
  } finally {
    await plain.close();
  }
});

test("the gateway's coarse unsigned 401 is unauthorized, and is not retried", async () => {
  await withServer(
    () => ok(SEND_RESULT),
    async (server, client) => {
      // Nothing registered for this key: the gateway answers the unsigned 401.
      const stranger = new VsmsClient({
        ...server.clientOptions,
        privateKey: newTestIdentity().clientSeed,
      });
      const error = await rejection(stranger.sendMessage(INPUT));
      assert.equal(error.kind, "unsigned");
      assert.equal(error.isUnauthorized(), true);
      assert.equal(server.requests.length, 1, "exactly one attempt");
      assert.equal(server.opened.length, 0, "the handler never ran");
      // The registered key still works: the refusal changed nothing.
      await client.getMessage("m1");
    },
  );
});

test("a request addressed to another audience is refused and the handler does not run", async () => {
  await withServer(
    () => ok(SEND_RESULT),
    async (server, client) => {
      const error = await rejection(client.sendMessage(INPUT));
      assert.equal(error.isUnauthorized(), true);
      assert.equal(server.opened.length, 0);
    },
    { audience: "someone-else" },
  );
});

test("an answer sealed by a key that is not pinned is unverified", async () => {
  const stranger = newTestIdentity();
  await withServer(
    () => ok(SEND_RESULT),
    async (_server, client) => {
      const error = await rejection(client.sendMessage(INPUT));
      assert.equal(error.kind, "unverified");
    },
    { answerSeed: stranger.serverSeed },
  );
});

test("an answer sealed by the next key is trusted, so a rotation is not a flag day", async () => {
  const identity = newTestIdentity();
  await withServer(
    () => ok(SEND_RESULT),
    async (_server, client) => {
      assert.equal((await client.sendMessage(INPUT)).result.messageId, "m1");
    },
    { identity, answerSeed: identity.nextSeed },
  );
});

// ------------------------------------------------------------ the transport

test("a redirect is not followed: the signed body goes to one host only", async () => {
  const elsewhere = await startTestServer((_request, response) => {
    response.writeHead(200, { "content-type": "application/json" });
    response.end("{}");
  });
  const redirecting = await startTestServer((_request, response) => {
    response.writeHead(307, { location: `${elsewhere.url}/$procs/sendMessage` });
    response.end();
  });
  try {
    const identity = newTestIdentity();
    const client = new VsmsClient({
      baseUrl: redirecting.url,
      privateKey: identity.clientSeed,
      serverKeys: { current: identity.serverPublic, next: identity.nextPublic },
    });
    const error = await rejection(client.sendMessage(INPUT));
    assert.equal(error.kind, "unsigned");
    assert.equal(error.httpStatus, 307);
    assert.equal(redirecting.requests.length, 1);
    assert.equal(elsewhere.requests.length, 0, "the redirect target was never contacted");
  } finally {
    await redirecting.close();
    await elsewhere.close();
  }
});

test("an answer longer than the cap is refused, declared or chunked", async () => {
  const identity = newTestIdentity();
  const options = {
    privateKey: identity.clientSeed,
    serverKeys: { current: identity.serverPublic, next: identity.nextPublic },
  };

  const declared = createServer((_req, res) => {
    res.writeHead(200, { "content-length": String(MAX_RESPONSE_BYTES + 1) });
    res.write("x");
  });
  await new Promise<void>((resolve) => declared.listen(0, "127.0.0.1", resolve));
  try {
    const client = new VsmsClient({
      ...options,
      baseUrl: `http://127.0.0.1:${(declared.address() as AddressInfo).port}`,
    });
    assert.equal((await rejection(client.sendMessage(INPUT))).kind, "response_too_large");
  } finally {
    declared.closeAllConnections();
    declared.close();
  }

  const chunked = createServer((_req, res) => {
    res.writeHead(200);
    const chunk = Buffer.alloc(1024 * 1024, 120);
    for (let sent = 0; sent <= MAX_RESPONSE_BYTES; sent += chunk.length) res.write(chunk);
    res.end();
  });
  await new Promise<void>((resolve) => chunked.listen(0, "127.0.0.1", resolve));
  try {
    const client = new VsmsClient({
      ...options,
      baseUrl: `http://127.0.0.1:${(chunked.address() as AddressInfo).port}`,
    });
    assert.equal((await rejection(client.sendMessage(INPUT))).kind, "response_too_large");
  } finally {
    chunked.closeAllConnections();
    chunked.close();
  }
});

test("a write is attempted once: a severed connection is not retried", async () => {
  let attempts = 0;
  // Reads the request, then drops the socket without an answer.
  const server = createServer((req, res) => {
    attempts += 1;
    req.resume();
    res.socket?.destroy();
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  try {
    const identity = newTestIdentity();
    const client = new VsmsClient({
      baseUrl: `http://127.0.0.1:${(server.address() as AddressInfo).port}`,
      privateKey: identity.clientSeed,
      serverKeys: { current: identity.serverPublic, next: identity.nextPublic },
    });
    const error = await rejection(client.sendMessage(INPUT, { idempotencyKey: "k" }));
    assert.equal(error.kind, "transport");
    assert.equal(attempts, 1, "no automatic retry of a non-idempotent call");
  } finally {
    server.closeAllConnections();
    server.close();
  }
});

test("a call that outlives its timeout is a transport error caused by the timeout", async () => {
  const stalled = await startTestServer(() => {
    // never answers
  });
  try {
    const identity = newTestIdentity();
    const client = new VsmsClient({
      baseUrl: stalled.url,
      privateKey: identity.clientSeed,
      serverKeys: { current: identity.serverPublic, next: identity.nextPublic },
      timeoutMs: 150,
    });
    const error = await rejection(client.getMessage("m1"));
    assert.equal(error.kind, "transport");
    assert.equal((error.cause as Error).name, "TimeoutError");
  } finally {
    await stalled.close();
  }
});

test("an unreachable gateway is a transport error", async () => {
  const identity = newTestIdentity();
  const client = new VsmsClient({
    baseUrl: "http://127.0.0.1:1",
    privateKey: identity.clientSeed,
    serverKeys: { current: identity.serverPublic, next: identity.nextPublic },
  });
  assert.equal((await rejection(client.previewMessage({ body: "test" }))).kind, "transport");
});

// ------------------------------------------------------------ the key

function ed25519Pem(): { pem: string; publicKey: string } {
  const { privateKey } = generateKeyPairSync("ed25519");
  const jwk = createPublicKey(privateKey).export({ format: "jwk" });
  return {
    pem: privateKey.export({ type: "pkcs8", format: "pem" }) as string,
    publicKey: jwk.x as string,
  };
}

/** A client over `key`, whose public half is registered at a fresh server. */
async function signedWith(
  key: (
    identity: ReturnType<typeof newTestIdentity>,
  ) => Partial<ConstructorParameters<typeof VsmsClient>[0]>,
  registered: (identity: ReturnType<typeof newTestIdentity>) => string,
): Promise<{ sent: boolean; publicKey: string }> {
  const identity = newTestIdentity();
  identity.clientPublic = registered(identity);
  const server = await startSealedServer(() => ok(SEND_RESULT), { identity });
  try {
    const client = new VsmsClient({
      baseUrl: server.url,
      serverKeys: { current: identity.serverPublic, next: identity.nextPublic },
      ...key(identity),
    });
    await client.sendMessage(INPUT);
    return { sent: server.opened.length === 1, publicKey: client.publicKey };
  } finally {
    await server.close();
  }
}

test("a PKCS#8 PEM signs, and the key stays in Node", async () => {
  const { pem, publicKey } = ed25519Pem();
  const result = await signedWith(
    () => ({ privateKey: pem }),
    () => publicKey,
  );
  assert.equal(result.sent, true);
  assert.equal(result.publicKey, publicKey);
});

test("a KeyObject signs", async () => {
  const { privateKey } = generateKeyPairSync("ed25519");
  const publicKey = createPublicKey(privateKey).export({ format: "jwk" }).x as string;
  const result = await signedWith(
    () => ({ privateKey }),
    () => publicKey,
  );
  assert.equal(result.sent, true);
});

test("a 32-byte seed, as bytes or as base64url, signs", async () => {
  for (const form of ["bytes", "text"] as const) {
    const result = await signedWith(
      (identity) => ({
        privateKey:
          form === "bytes" ? Buffer.from(identity.clientSeed, "base64url") : identity.clientSeed,
      }),
      (identity) => identity.clientPublic,
    );
    assert.equal(result.sent, true, form);
  }
});

test("a key file signs", async () => {
  const { pem, publicKey } = ed25519Pem();
  const dir = mkdtempSync(join(tmpdir(), "vsms-key-"));
  try {
    const keyPath = join(dir, "signing-key.pem");
    writeFileSync(keyPath, pem, { mode: 0o600 });
    const result = await signedWith(
      () => ({ keyPath }),
      () => publicKey,
    );
    assert.equal(result.sent, true);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("a signer callback signs, and a signature by the wrong key is refused before it is sent", async () => {
  const { privateKey } = generateKeyPairSync("ed25519");
  const publicKey = createPublicKey(privateKey).export({ format: "jwk" }).x as string;
  const result = await signedWith(
    () => ({ signer: { publicKey, sign: (tbs) => sign(null, tbs, privateKey) } }),
    () => publicKey,
  );
  assert.equal(result.sent, true);

  // A signer whose key is not the one it announced.
  const { privateKey: other } = generateKeyPairSync("ed25519");
  const identity = newTestIdentity();
  identity.clientPublic = publicKey;
  const server = await startSealedServer(() => ok(SEND_RESULT), { identity });
  try {
    const client = new VsmsClient({
      baseUrl: server.url,
      serverKeys: { current: identity.serverPublic, next: identity.nextPublic },
      signer: { publicKey, sign: (tbs) => sign(null, tbs, other) },
    });
    const error = await rejection(client.sendMessage(INPUT));
    assert.equal(error.kind, "seal");
    assert.match(error.message, /does not verify under the public key/);
    assert.equal(server.requests.length, 0, "nothing was sent");
  } finally {
    await server.close();
  }
});

// ------------------------------------------------------- bad configuration

function build(options: Partial<ConstructorParameters<typeof VsmsClient>[0]>): VsmsClient {
  const identity = newTestIdentity();
  return new VsmsClient({
    baseUrl: "http://127.0.0.1:1",
    privateKey: identity.clientSeed,
    serverKeys: { current: identity.serverPublic, next: identity.nextPublic },
    ...options,
  });
}

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

test("a client without a usable key or pinned keys does not construct", () => {
  const identity = newTestIdentity();
  const keys = { current: identity.serverPublic, next: identity.nextPublic };
  const seed = identity.clientSeed;

  configError(() => new VsmsClient({ baseUrl: "http://x", serverKeys: keys }));
  configError(() => build({ keyPath: "/nonexistent/key.pem" }));
  configError(() => build({ privateKey: seed, keyPath: "/some/key.pem" }));
  // A seed of the wrong length, padded, or in the other alphabet.
  configError(() => build({ privateKey: "AAAA" }));
  configError(() => build({ privateKey: `${seed}=` }));
  configError(() => build({ privateKey: Buffer.alloc(31) }));
  // Pinned keys: missing, malformed, the same twice, one only.
  configError(() => build({ serverKeys: undefined as never }));
  configError(() => build({ serverKeys: { current: keys.current } as never }));
  configError(() => build({ serverKeys: { current: keys.current, next: "AAAA" } }));
  configError(() => build({ serverKeys: { current: keys.current, next: keys.current } }));
  // A point that is not on the curve.
  const notAPoint = Buffer.alloc(32);
  notAPoint[0] = 2;
  configError(() => build({ serverKeys: { current: keys.current, next: notAPoint } }));
  configError(() => build({ baseUrl: "" }));
  configError(() => build({ timeoutMs: 0 }));
});

test("only an Ed25519 key is an app client's: RSA and P-256 are refused, naming no key", () => {
  const rsa = generateKeyPairSync("rsa", { modulusLength: 2048 }).privateKey;
  const p256 = generateKeyPairSync("ec", { namedCurve: "P-256" }).privateKey;
  for (const key of [rsa, p256]) {
    const error = configError(() => build({ privateKey: key }));
    assert.match(error.message, /Ed25519/);
    const pem = key.export({ type: "pkcs8", format: "pem" }) as string;
    assert.match(configError(() => build({ privateKey: pem })).message, /Ed25519/);
  }
});

test("a configuration error never quotes the key it refused", () => {
  const identity = newTestIdentity();
  const seed = identity.clientSeed;
  const error = configError(() =>
    build({
      privateKey: seed,
      serverKeys: { current: seed.slice(0, 40), next: identity.nextPublic },
    }),
  );
  assert.equal(error.message.includes(seed.slice(0, 20)), false);
  const garbage = configError(() =>
    build({ privateKey: "-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----" }),
  );
  assert.equal(garbage.message.includes("AAAA"), false);
  assert.ok(garbage.cause, "OpenSSL's own reason is the cause");
});

test("an unreadable PEM is a config error", () => {
  assert.throws(() => createPrivateKey("nope"));
  configError(() =>
    build({ privateKey: "-----BEGIN PRIVATE KEY-----\nnot a key\n-----END PRIVATE KEY-----" }),
  );
});
