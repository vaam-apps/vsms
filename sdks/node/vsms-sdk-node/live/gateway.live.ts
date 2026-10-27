/**
 * Live verification against a real `sms-gateway serve`: not part of any CI job
 * (no GitHub job runs a gateway for the Node SDK; `ci.yml` builds and unit-tests
 * it). Run by hand:
 *
 *   # 1. A gateway started with its two envelope key files and a database
 *   #    (`sms-gateway serve --envelope-signing-key-file current.pem
 *   #     --envelope-next-signing-key-file next.pem ...`).
 *   # 2. A key whose public half is registered for an app client:
 *   #      openssl genpkey -algorithm ed25519 -out sdk-test-key.pem
 *   #      sms-gateway register-client --app <slug> --scope sms:send --scope sms:read \
 *   #        --public-key "$(openssl pkey -in sdk-test-key.pem -pubout -outform DER \
 *   #                          | tail -c 32 | base64 | tr '+/' '-_' | tr -d '=')"
 *   # 3. The gateway's pinned keys:
 *   #      sms-gateway server-keys --envelope-signing-key-file current.pem \
 *   #        --envelope-next-signing-key-file next.pem > server-keys.txt
 *
 *   VSMS_SDK_TEST_SIGNING_KEY_PATH=sdk-test-key.pem \
 *   VSMS_SDK_TEST_SERVER_KEYS_PATH=server-keys.txt \
 *   pnpm --filter @vaam-apps/vsms-node run test:live
 *
 * `VSMS_SDK_TEST_BASE_URL` (default `http://127.0.0.1:8080`) overrides the
 * origin. The two key paths have no defaults: they are per-run material, so the
 * run fails with a message pointing back here if one is missing.
 *
 * This is the SDK's own version of `vsms-sdk-rust`'s `tests/live_gateway.rs`:
 * a sealed call really works against the real `EnvelopeLayer`; a replayed
 * `Idempotency-Key` does not send twice; a refused signature is surfaced as the
 * unsigned 401 (that it is attempted once is `test/client.test.ts`'s, which can
 * count requests); and an answer under keys that are not pinned is not
 * believed.
 */
import { strict as assert } from "node:assert";
import { createPublicKey, generateKeyPairSync, randomBytes } from "node:crypto";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { parseServerKeysReport, SdkError, VsmsClient } from "../dist/index.js";

function required(name: string): string {
  const value = process.env[name];
  if (!value) {
    throw new Error(`${name} is required for this live test: see this file's own header`);
  }
  return value;
}

const baseUrl = process.env.VSMS_SDK_TEST_BASE_URL ?? "http://127.0.0.1:8080";
const keyPath = required("VSMS_SDK_TEST_SIGNING_KEY_PATH");
const serverKeys = parseServerKeysReport(
  readFileSync(required("VSMS_SDK_TEST_SERVER_KEYS_PATH"), "utf8"),
);

const client = new VsmsClient({ baseUrl, keyPath, serverKeys });

/** A fresh Ed25519 public key, as the unpadded base64url a client pins. */
function freshPublicKey(): string {
  const { privateKey } = generateKeyPairSync("ed25519");
  return createPublicKey(privateKey).export({ format: "jwk" }).x as string;
}

test("a sealed preview reaches the real gateway and its sealed answer verifies", async () => {
  const preview = await client.previewMessage({ body: "Your code is 4471", to: "+237677123456" });
  assert.equal(preview.encoding, "gsm7");
  assert.equal(preview.segments, 1);
});

test("a send is read back by id, and a replayed Idempotency-Key sends once", async () => {
  const key = `sdk-live-${Date.now()}`;
  const input = { to: "+237677123456", body: "sdk live test", senderId: "VYMALO" };

  const first = await client.sendMessage(input, { idempotencyKey: key });
  assert.equal(first.idempotencyReplayed, false);

  const second = await client.sendMessage(input, { idempotencyKey: key });
  assert.equal(second.result.messageId, first.result.messageId, "no second Message row");
  assert.equal(second.idempotencyReplayed, true);

  const message = await client.getMessage(first.result.messageId);
  assert.equal(message.id, first.result.messageId);
  // The row's own field is `msisdn` (types.ts still says `to`; it is read
  // through the index signature here, so this test pins the wire, not the type).
  assert.equal(message.msisdn, input.to);
});

test("a signature from an unregistered key is refused as the coarse unsigned 401", async () => {
  const stranger = new VsmsClient({
    baseUrl,
    privateKey: randomBytes(32).toString("base64url"),
    serverKeys,
  });
  await assert.rejects(stranger.getMessage("m1"), (err: unknown) => {
    assert.ok(err instanceof SdkError);
    assert.equal(err.kind, "unsigned");
    assert.equal(err.isUnauthorized(), true);
    return true;
  });
});

test("an answer is not believed under keys the client did not pin", async () => {
  const wrong = new VsmsClient({
    baseUrl,
    keyPath,
    serverKeys: { current: freshPublicKey(), next: freshPublicKey() },
  });
  await assert.rejects(wrong.previewMessage({ body: "x" }), (err: unknown) => {
    assert.ok(err instanceof SdkError);
    assert.equal(err.kind, "unverified");
    return true;
  });
});
