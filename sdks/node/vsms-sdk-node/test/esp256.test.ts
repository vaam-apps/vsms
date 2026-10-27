import { strict as assert } from "node:assert";
import { createPublicKey, generateKeyPairSync, sign, webcrypto } from "node:crypto";
import { test } from "node:test";
import { loadWasm } from "../dist/wasm.js";
import { loadResponderModule, newTestIdentity } from "../support/responder.ts";

// The operator console signs with a P-256 key a browser holds as a WebCrypto
// non-extractable `CryptoKey` (VS-5b). The key never leaves WebCrypto: the
// sealing module sees the bytes to sign and the signature, and nothing else.

const SEND = JSON.stringify({ args: { to: "+237677123456", body: "hi" } });

function server(publicKeys: string[], identity = newTestIdentity()) {
  const responder = new (loadResponderModule().Responder)(identity.serverSeed, publicKeys, "vsms");
  return { identity, responder };
}

test("a WebCrypto non-extractable P-256 key seals a call the server verifies", async () => {
  const pair = await webcrypto.subtle.generateKey({ name: "ECDSA", namedCurve: "P-256" }, false, [
    "sign",
    "verify",
  ]);
  const raw = Buffer.from(await webcrypto.subtle.exportKey("raw", pair.publicKey));
  assert.equal(raw.length, 65);
  assert.equal(raw[0], 4);
  const publicKey = raw.toString("base64url");

  const { identity, responder } = server([publicKey]);
  const sealer = loadWasm().Sealer.fromEsp256Callback(
    publicKey,
    async (tbs) =>
      new Uint8Array(
        await webcrypto.subtle.sign(
          { name: "ECDSA", hash: "SHA-256" },
          pair.privateKey,
          new Uint8Array(tbs),
        ),
      ),
    identity.serverPublic,
    identity.nextPublic,
  );
  assert.equal(sealer.algorithm, "esp256");
  assert.equal(sealer.publicKey, publicKey);

  const sealed = await sealer.sealCall("POST", "/$procs/sendMessage", [], SEND, undefined, "k-1");
  const opened = await responder.openRequest(
    "POST",
    sealed.target,
    undefined,
    sealed.headers,
    sealed.body,
  );
  assert.equal(opened.payload, SEND);
  assert.equal(opened.idempotencyKey, "k-1");

  const answer = await opened.sealResponse(200, '{"messageId":"m1"}');
  assert.equal(await sealed.openResponse(200, answer.headers, answer.body), '{"messageId":"m1"}');
});

test("a DER signature from Node's crypto is accepted too", async () => {
  const { privateKey } = generateKeyPairSync("ec", { namedCurve: "P-256" });
  const jwk = createPublicKey(privateKey).export({ format: "jwk" });
  const publicKey = Buffer.concat([
    Buffer.from([4]),
    Buffer.from(jwk.x as string, "base64url"),
    Buffer.from(jwk.y as string, "base64url"),
  ]).toString("base64url");

  const { identity, responder } = server([publicKey]);
  const sealer = loadWasm().Sealer.fromEsp256Callback(
    publicKey,
    (tbs) => sign("sha256", tbs, privateKey),
    identity.serverPublic,
    identity.nextPublic,
  );
  const sealed = await sealer.sealCall(
    "GET",
    "/messages/{id}",
    ["m1"],
    undefined,
    undefined,
    undefined,
  );
  assert.ok(
    await responder.openRequest("GET", sealed.target, undefined, sealed.headers, sealed.body),
  );
});

test("an ESP256 request from a key the server holds only as Ed25519 is refused", async () => {
  const { privateKey } = generateKeyPairSync("ec", { namedCurve: "P-256" });
  const jwk = createPublicKey(privateKey).export({ format: "jwk" });
  const publicKey = Buffer.concat([
    Buffer.from([4]),
    Buffer.from(jwk.x as string, "base64url"),
    Buffer.from(jwk.y as string, "base64url"),
  ]).toString("base64url");

  // The gateway reads `ClientKey` for Ed25519 and `ConsoleKey` for ESP256: a
  // server that registered only an Ed25519 key never resolves this one.
  const identity = newTestIdentity();
  const { responder } = server([identity.clientPublic], identity);
  const sealer = loadWasm().Sealer.fromEsp256Callback(
    publicKey,
    (tbs) => sign("sha256", tbs, { key: privateKey, dsaEncoding: "ieee-p1363" }),
    identity.serverPublic,
    identity.nextPublic,
  );
  const sealed = await sealer.sealCall(
    "POST",
    "/$procs/sendMessage",
    [],
    SEND,
    undefined,
    undefined,
  );
  await assert.rejects(
    responder.openRequest("POST", sealed.target, undefined, sealed.headers, sealed.body),
  );
});

test("a P-256 callback whose signature is by another key is refused before it is sent", async () => {
  const { privateKey } = generateKeyPairSync("ec", { namedCurve: "P-256" });
  const { privateKey: other } = generateKeyPairSync("ec", { namedCurve: "P-256" });
  const jwk = createPublicKey(privateKey).export({ format: "jwk" });
  const publicKey = Buffer.concat([
    Buffer.from([4]),
    Buffer.from(jwk.x as string, "base64url"),
    Buffer.from(jwk.y as string, "base64url"),
  ]).toString("base64url");
  const identity = newTestIdentity();
  const sealer = loadWasm().Sealer.fromEsp256Callback(
    publicKey,
    (tbs) => sign("sha256", tbs, { key: other, dsaEncoding: "ieee-p1363" }),
    identity.serverPublic,
    identity.nextPublic,
  );
  await assert.rejects(
    sealer.sealCall("POST", "/$procs/sendMessage", [], SEND, undefined, undefined),
    (failure: { kind: string; message: string }) =>
      failure.kind === "seal" && failure.message.includes("does not verify under the public key"),
  );
});

test("a public key that is not a P-256 point is refused at construction", () => {
  const identity = newTestIdentity();
  assert.throws(
    () =>
      loadWasm().Sealer.fromEsp256Callback(
        Buffer.alloc(65, 4).toString("base64url"),
        () => new Uint8Array(64),
        identity.serverPublic,
        identity.nextPublic,
      ),
    (failure: { kind: string }) => failure.kind === "invalid",
  );
});
