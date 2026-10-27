import { strict as assert } from "node:assert";
import { createPublicKey, generateKeyPairSync, sign } from "node:crypto";
import { test } from "node:test";
import { VsmsClient } from "../dist/index.js";
import { loadWasm } from "../dist/wasm.js";
import { INPUT, ok, rejection, SEND_RESULT } from "../support/helpers.ts";
import { newTestIdentity } from "../support/responder.ts";
import { startSealedServer, startTestServer } from "../support/server.ts";

// A signer is the caller's code: a KMS client, an HSM binding. It can return
// the wrong thing, throw, or never answer. None of that may take the process
// down, outlive the caller's timeout, or leave the client unusable.

function ed25519() {
  const { privateKey } = generateKeyPairSync("ed25519");
  const publicKey = createPublicKey(privateKey).export({ format: "jwk" }).x as string;
  return { publicKey, sign: (tbs: Uint8Array) => sign(null, tbs, privateKey) };
}

/** An object that throws as soon as anything reads `length`, the way a broken array-like does. */
const THROWING_LENGTH = Object.defineProperty({}, "length", {
  get() {
    throw new Error("length");
  },
});

// Every value a callback might return that is not a signature as bytes.
const NOT_BYTES: [string, () => unknown][] = [
  ["a negative number", () => -1],
  ["a number too large to allocate", () => 1e10],
  ["a string", () => "AAAA"],
  ["null", () => null],
  ["undefined", () => undefined],
  ["a plain array", () => [1, 2, 3]],
  ["an object whose length getter throws", () => THROWING_LENGTH],
  ["an array-like of the right length", () => ({ length: 64 })],
  ["a signature far too long to be one", () => new Uint8Array(4096)],
];

// ------------------------------------------------------ through the module

for (const [label, value] of NOT_BYTES) {
  test(`the sealing module refuses ${label} from a signing callback, as a seal failure`, async () => {
    const identity = newTestIdentity();
    const key = ed25519();
    let calls = 0;
    const sealer = loadWasm().Sealer.fromCallback(
      key.publicKey,
      (tbs) => {
        calls += 1;
        // The first call misbehaves; the next ones sign properly, which shows
        // the same sealer is still usable afterwards.
        return calls === 1 ? (value() as Uint8Array) : key.sign(tbs);
      },
      identity.serverPublic,
      identity.nextPublic,
    );
    await assert.rejects(
      sealer.sealCall("POST", "/$procs/sendMessage", [], "{}", undefined, undefined),
      (failure: { kind: string }) => failure.kind === "seal",
    );
    const sealed = await sealer.sealCall(
      "POST",
      "/$procs/sendMessage",
      [],
      "{}",
      undefined,
      undefined,
    );
    assert.ok(sealed.body.length > 0, "the sealer still seals");
    assert.equal(calls, 2);
  });
}

test("the sealing module takes an ArrayBuffer of the signature too", async () => {
  const identity = newTestIdentity();
  const key = ed25519();
  const sealer = loadWasm().Sealer.fromCallback(
    key.publicKey,
    (tbs) => {
      const signature = key.sign(tbs);
      return signature.buffer.slice(
        signature.byteOffset,
        signature.byteOffset + signature.byteLength,
      ) as unknown as Uint8Array;
    },
    identity.serverPublic,
    identity.nextPublic,
  );
  const sealed = await sealer.sealCall(
    "POST",
    "/$procs/sendMessage",
    [],
    "{}",
    undefined,
    undefined,
  );
  assert.ok(sealed.body.length > 0);
});

test("a callback that throws, or rejects, is a seal failure and the sealer survives it", async () => {
  const identity = newTestIdentity();
  const key = ed25519();
  const behaviours: ((tbs: Uint8Array) => Uint8Array | Promise<Uint8Array>)[] = [
    () => {
      throw new Error("boom");
    },
    () => Promise.reject(new Error("boom")),
    key.sign,
  ];
  const sealer = loadWasm().Sealer.fromCallback(
    key.publicKey,
    (tbs) => (behaviours.shift() as (tbs: Uint8Array) => Uint8Array)(tbs),
    identity.serverPublic,
    identity.nextPublic,
  );
  for (let i = 0; i < 2; i += 1) {
    await assert.rejects(
      sealer.sealCall("POST", "/$procs/sendMessage", [], "{}", undefined, undefined),
      (failure: { kind: string }) => failure.kind === "seal",
    );
  }
  assert.ok(
    (await sealer.sealCall("POST", "/$procs/sendMessage", [], "{}", undefined, undefined)).body,
  );
});

// ------------------------------------------------------- through the client

for (const [label, value] of NOT_BYTES) {
  test(`the client refuses ${label} from a signer before anything is sent`, async () => {
    const key = ed25519();
    const identity = newTestIdentity();
    identity.clientPublic = key.publicKey;
    const server = await startSealedServer(() => ok(SEND_RESULT), { identity });
    try {
      let calls = 0;
      const client = new VsmsClient({
        baseUrl: server.url,
        serverKeys: { current: identity.serverPublic, next: identity.nextPublic },
        signer: {
          publicKey: key.publicKey,
          sign: (tbs) => {
            calls += 1;
            return calls === 1 ? (value() as Uint8Array) : key.sign(tbs);
          },
        },
      });
      const error = await rejection(client.sendMessage(INPUT));
      assert.equal(error.kind, "seal");
      assert.equal(server.requests.length, 0, "nothing was sent");
      // The same client signs the next call.
      assert.deepEqual((await client.sendMessage(INPUT)).result, SEND_RESULT);
    } finally {
      await server.close();
    }
  });
}

test("an ArrayBuffer from a signer is refused by the client, which asks for bytes", async () => {
  const key = ed25519();
  const identity = newTestIdentity();
  identity.clientPublic = key.publicKey;
  const server = await startSealedServer(() => ok(SEND_RESULT), { identity });
  try {
    const client = new VsmsClient({
      baseUrl: server.url,
      serverKeys: { current: identity.serverPublic, next: identity.nextPublic },
      signer: {
        publicKey: key.publicKey,
        sign: (tbs) => key.sign(tbs).buffer as unknown as Uint8Array,
      },
    });
    assert.equal((await rejection(client.sendMessage(INPUT))).kind, "seal");
    assert.equal(server.requests.length, 0);
  } finally {
    await server.close();
  }
});

test("a signer that never answers is bounded by timeoutMs, and the client signs afterwards", async () => {
  const key = ed25519();
  const identity = newTestIdentity();
  identity.clientPublic = key.publicKey;
  const server = await startSealedServer(() => ok(SEND_RESULT), { identity });
  try {
    let calls = 0;
    const client = new VsmsClient({
      baseUrl: server.url,
      timeoutMs: 300,
      serverKeys: { current: identity.serverPublic, next: identity.nextPublic },
      signer: {
        publicKey: key.publicKey,
        sign: (tbs) => {
          calls += 1;
          // A hung KMS the first time, a healthy one after it.
          return calls === 1 ? new Promise<Uint8Array>(() => undefined) : key.sign(tbs);
        },
      },
    });
    const started = Date.now();
    const error = await rejection(client.sendMessage(INPUT));
    const elapsed = Date.now() - started;
    assert.equal(error.kind, "seal");
    assert.match(error.message, /300 ms/);
    assert.ok(elapsed < 2_000, `rejected after ${elapsed} ms, not at the deadline`);
    assert.equal(server.requests.length, 0, "nothing was sent");
    assert.deepEqual((await client.sendMessage(INPUT)).result, SEND_RESULT);
  } finally {
    await server.close();
  }
});

test("the deadline covers signing and sending together, not each on its own", async () => {
  const key = ed25519();
  const identity = newTestIdentity();
  // A gateway that accepts the connection and never answers.
  const stalled = await startTestServer(() => undefined);
  try {
    const client = new VsmsClient({
      baseUrl: stalled.url,
      timeoutMs: 500,
      serverKeys: { current: identity.serverPublic, next: identity.nextPublic },
      signer: {
        publicKey: key.publicKey,
        sign: async (tbs) => {
          // Half the budget goes to signing.
          await new Promise((resolve) => setTimeout(resolve, 250));
          return key.sign(tbs);
        },
      },
    });
    const started = Date.now();
    const error = await rejection(client.sendMessage(INPUT));
    const elapsed = Date.now() - started;
    assert.equal(error.kind, "transport");
    assert.equal(stalled.requests.length, 1, "it was signed in time, and sent");
    // 250 ms signing + the 250 ms that were left; a full second budget for the
    // request would put this at 750.
    assert.ok(elapsed < 700, `took ${elapsed} ms`);
  } finally {
    await stalled.close();
  }
});
