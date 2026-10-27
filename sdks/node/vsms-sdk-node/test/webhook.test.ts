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
const ENVELOPE = JSON.stringify({
  id: "cattempt00000000000000",
  eventId: "11111111-1111-4111-8111-111111111111",
  type: "message.delivered",
  occurredAt: "2026-10-27T10:00:00Z",
  data: { messageId: "cmsg000000000000000000", state: "delivered" },
});

function opener(
  identity = newTestIdentity(),
  overrides: Partial<{ audience: string; route: string }> = {},
) {
  return {
    identity,
    opener: new WebhookOpener({
      serverKeys: { current: identity.serverPublic, next: identity.nextPublic },
      audience: overrides.audience ?? AUDIENCE,
      route: overrides.route ?? ROUTE,
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
  ];
  for (const delivery of wrong) await assert.rejects(o.open(delivery), unverified);
  const tampered = await seal(identity.serverSeed);
  tampered[tampered.length - 1] = (tampered[tampered.length - 1] ?? 0) ^ 1;
  await assert.rejects(o.open(tampered), unverified);
  await assert.rejects(o.open("not a cose message"), unverified);
  await assert.rejects(o.open(new Uint8Array()), unverified);
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
  assert.throws(() => new WebhookOpener({ serverKeys, audience: "", route: ROUTE }), config);
  assert.throws(
    () => new WebhookOpener({ serverKeys, audience: AUDIENCE, route: "no-slash" }),
    config,
  );
  assert.throws(
    () =>
      new WebhookOpener({
        serverKeys: { current: identity.serverPublic, next: identity.serverPublic },
        audience: AUDIENCE,
        route: ROUTE,
      }),
    config,
  );
  assert.throws(
    () => new WebhookOpener({ serverKeys: undefined as never, audience: AUDIENCE, route: ROUTE }),
    config,
  );
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
    nowSeconds: Number(vector.iat),
  });
  const event = await o.open(Buffer.from(String(vector.sealedHex), "hex"));
  const plaintext = JSON.parse(String(vector.plaintext));
  assert.equal(event.id, plaintext.id);
  assert.equal(event.eventId, plaintext.eventId);
  assert.deepEqual(event.data, plaintext.data);
});
