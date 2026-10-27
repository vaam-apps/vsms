import { strict as assert } from "node:assert";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { SdkError, WebhookOpener } from "../dist/index.js";
import { loadResponderModule, newTestIdentity } from "../support/responder.ts";

// vsms delivers an event as a sealed request signed with its own envelope
// signing key. These tests seal deliveries the way the gateway's worker does
// (cratestack's ClientEnvelope, through the test build's `sealDelivery`) and
// open them through the module the package ships; the committed vector is bytes
// the gateway's own `sms-webhook` crate produced.

const AUDIENCE = "vaam";
const ROUTE = "/webhooks/vsms";
const APP_ID = "capp00000000000000000";
const ENDPOINT_ID = "cendpoint0000000000000";
const envelope = (appId: string) =>
  JSON.stringify({
    id: "cattempt00000000000000",
    eventId: "11111111-1111-4111-8111-111111111111",
    type: "message.delivered",
    occurredAt: "2026-10-27T10:00:00Z",
    appId,
    endpointId: ENDPOINT_ID,
    data: { messageId: "cmsg000000000000000000", state: "delivered" },
  });
const ENVELOPE = envelope(APP_ID);

function opener(
  identity = newTestIdentity(),
  overrides: Partial<{ audience: string; route: string; appId: string; endpointId: string }> = {},
) {
  return {
    identity,
    opener: new WebhookOpener({
      serverKeys: { current: identity.serverPublic, next: identity.nextPublic },
      audience: overrides.audience ?? AUDIENCE,
      route: overrides.route ?? ROUTE,
      appId: overrides.appId ?? APP_ID,
      endpointId: overrides.endpointId,
    }),
  };
}

const seal = (
  seed: string,
  options: {
    audience?: string;
    route?: string;
    contractHex?: string;
    iat?: number;
    body?: string;
  } = {},
) =>
  loadResponderModule().sealDelivery(
    seed,
    options.audience ?? AUDIENCE,
    options.route ?? ROUTE,
    options.body ?? ENVELOPE,
    options.contractHex,
    options.iat,
  );

const unverified = (error: unknown) => error instanceof SdkError && error.kind === "unverified";
const now = () => Math.floor(Date.now() / 1000);

test("a genuine delivery opens into the typed event", async () => {
  const { identity, opener: o } = opener();
  const event = await o.open(await seal(identity.serverSeed));
  assert.equal(event.id, "cattempt00000000000000");
  assert.equal(event.eventId, "11111111-1111-4111-8111-111111111111");
  assert.equal(event.type, "message.delivered");
  assert.equal(event.occurredAt, "2026-10-27T10:00:00Z");
  assert.equal(event.appId, APP_ID);
  assert.equal(event.endpointId, ENDPOINT_ID);
  assert.deepEqual(event.data, { messageId: "cmsg000000000000000000", state: "delivered" });
});

test("the next pinned key opens too, so a rotation is not a flag day", async () => {
  const { identity, opener: o } = opener();
  assert.equal((await o.open(await seal(identity.nextSeed))).type, "message.delivered");
});

test("every wrong delivery is the one coarse refusal", async () => {
  const { identity, opener: o } = opener();
  const wrong = [
    await seal(identity.serverSeed, { audience: "someone-else" }),
    await seal(identity.serverSeed, { route: "/webhooks/other" }),
    await seal(newTestIdentity().serverSeed),
    await seal(identity.serverSeed, { contractHex: "09".repeat(32) }),
    await seal(identity.serverSeed, { iat: now() - 3600 }),
    // Verifies in every other respect: made for another app's endpoint.
    await seal(identity.serverSeed, { body: envelope("cappother000000000000") }),
  ];
  for (const delivery of wrong) await assert.rejects(o.open(delivery), unverified);
  const tampered = await seal(identity.serverSeed);
  tampered[tampered.length - 1] = (tampered[tampered.length - 1] ?? 0) ^ 1;
  await assert.rejects(o.open(tampered), unverified);
  await assert.rejects(o.open(new TextEncoder().encode("not a cose message")), unverified);
  await assert.rejects(o.open(new Uint8Array()), unverified);
});

test("an endpoint is checked only when the opener is given one", async () => {
  const { identity, opener: other } = opener(undefined, { endpointId: "cendpointother0000000" });
  await assert.rejects(other.open(await seal(identity.serverSeed)), unverified);
  const { identity: id2, opener: matching } = opener(undefined, { endpointId: ENDPOINT_ID });
  assert.equal((await matching.open(await seal(id2.serverSeed))).endpointId, ENDPOINT_ID);
});

test("an app id rewritten in the sealed bytes fails the signature", async () => {
  const { identity, opener: o } = opener();
  const sealed = await seal(identity.serverSeed, { body: envelope("cappother000000000000") });
  const from = Buffer.from("cappother000000000000");
  const at = Buffer.from(sealed).indexOf(from);
  assert.notEqual(at, -1, "the app id is in the signed bytes");
  Buffer.from(APP_ID).copy(sealed, at);
  await assert.rejects(o.open(sealed), unverified);
});

test("a string body is refused: the delivery must be the raw bytes", async () => {
  const { identity, opener: o } = opener();
  const sealed = await seal(identity.serverSeed);
  const text = Buffer.from(sealed).toString("utf8");
  await assert.rejects(
    o.open(text as never),
    (error: unknown) => error instanceof SdkError && error.kind === "config",
  );
});

test("a replay is refused and a retry, sealed afresh, is not", async () => {
  const { identity, opener: o } = opener();
  const first = await seal(identity.serverSeed);
  await o.open(first);
  await assert.rejects(o.open(first), unverified);
  await o.open(await seal(identity.serverSeed));
});

test("a verified body that is not the envelope is a decode error, not a refusal", async () => {
  const { identity, opener: o } = opener();
  await assert.rejects(
    o.open(await seal(identity.serverSeed, { body: '{"hello":"world"}' })),
    (error: unknown) => error instanceof SdkError && error.kind === "decode",
  );
  await assert.rejects(
    o.open(await seal(identity.serverSeed, { body: "not json" })),
    (error: unknown) => error instanceof SdkError && error.kind === "decode",
  );
});

test("a misconfigured opener is refused before anything is opened", () => {
  const identity = newTestIdentity();
  const serverKeys = { current: identity.serverPublic, next: identity.nextPublic };
  const config = (error: unknown) => error instanceof SdkError && error.kind === "config";
  const base = { serverKeys, audience: AUDIENCE, route: ROUTE, appId: APP_ID };
  assert.throws(() => new WebhookOpener({ ...base, audience: "" }), config);
  assert.throws(() => new WebhookOpener({ ...base, route: "no-slash" }), config);
  // No default accepts any app: an opener without an app id cannot be built.
  assert.throws(() => new WebhookOpener({ ...base, appId: "" }), config);
  assert.throws(() => new WebhookOpener({ ...base, appId: undefined as never }), config);
  assert.throws(() => new WebhookOpener({ ...base, endpointId: "" }), config);
  assert.throws(
    () =>
      new WebhookOpener({
        ...base,
        serverKeys: { current: identity.serverPublic, next: identity.serverPublic },
      }),
    config,
  );
  assert.throws(() => new WebhookOpener({ ...base, serverKeys: undefined as never }), config);
});

test("the gateway's own vector opens", async () => {
  const vector = JSON.parse(
    readFileSync(
      new URL(
        "../../../rust/vsms-sdk-rust/tests/fixtures/sealed_delivery_vector.json",
        import.meta.url,
      ),
      "utf8",
    ),
  ) as Record<string, string | number>;
  const current = Buffer.from(String(vector.publicKeyHex), "hex").toString("base64url");
  const next = newTestIdentity().nextPublic;
  const o = new WebhookOpener({
    serverKeys: { current, next },
    audience: String(vector.audience),
    route: String(vector.route),
    appId: String(vector.appId),
    endpointId: String(vector.endpointId),
    nowSeconds: Number(vector.iat),
  });
  const event = await o.open(Buffer.from(String(vector.sealedHex), "hex"));
  const plaintext = JSON.parse(String(vector.plaintext));
  assert.equal(event.id, plaintext.id);
  assert.equal(event.eventId, plaintext.eventId);
  assert.equal(event.appId, vector.appId);
  assert.equal(event.endpointId, vector.endpointId);
  assert.deepEqual(event.data, plaintext.data);
});
