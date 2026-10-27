import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import type { AddressInfo } from "node:net";
import { after, describe, test } from "node:test";
import { fileURLToPath } from "node:url";
import { SdkError, type WebhookEvent, WebhookOpener } from "@vaam-apps/vsms-node";
import { ROUTE, sealedOpener } from "./config.ts";
import { HmacOpener } from "./hmac-opener.ts";
import { createReceiver, type EventOpener } from "./server.ts";
import { computeSignature } from "./signature.ts";
import type { ProcessedResult } from "./types.ts";

/**
 * The receiver against (1) the real `WebhookOpener` and bytes the gateway's
 * own `sms-webhook` crate sealed, and (2) a stub opener, to drive the
 * dedupe/ordering logic with events the test chooses.
 */

const VECTOR = JSON.parse(
  readFileSync(
    fileURLToPath(
      new URL(
        "../../../../sdks/rust/vsms-sdk-rust/tests/fixtures/sealed_delivery_vector.json",
        import.meta.url,
      ),
    ),
    "utf8",
  ),
) as {
  publicKeyHex: string;
  audience: string;
  route: string;
  appId: string;
  endpointId: string;
  iat: number;
  sealedHex: string;
  plaintext: string;
};

const NEXT_KEY = "PUAXw-hDiVqStwqnTRt-vJyYLM8uxJaMwM1V8Sr0Zgw";

async function started(opener: EventOpener) {
  const results: ProcessedResult[] = [];
  const receiver = createReceiver({
    port: 0,
    opener,
    simulatedWorkMs: 1,
    onProcessed: (r) => results.push(r),
  });
  const server = await receiver.listen();
  const port = (server.address() as AddressInfo).port;
  const post = async (body: Uint8Array): Promise<number> =>
    (
      await fetch(`http://127.0.0.1:${port}${ROUTE}`, {
        method: "POST",
        headers: { "content-type": "application/cose" },
        body: new Uint8Array(body),
      })
    ).status;
  const settle = () => new Promise((resolve) => setTimeout(resolve, 80));
  after(() => server.close());
  return { receiver, results, post, settle };
}

function realOpener(
  overrides: { audience?: string; appId?: string; endpointId?: string; nowSeconds?: number } = {},
): EventOpener {
  return sealedOpener(
    new WebhookOpener({
      serverKeys: {
        current: Buffer.from(VECTOR.publicKeyHex, "hex").toString("base64url"),
        next: NEXT_KEY,
      },
      audience: overrides.audience ?? VECTOR.audience,
      route: ROUTE,
      appId: overrides.appId ?? VECTOR.appId,
      endpointId: overrides.endpointId ?? VECTOR.endpointId,
      nowSeconds: overrides.nowSeconds ?? VECTOR.iat,
    }),
  );
}

describe("with the real WebhookOpener and a delivery the gateway's own sealer produced", () => {
  const genuine = Buffer.from(VECTOR.sealedHex, "hex");

  test("a genuine delivery is opened, acked and processed", async () => {
    const { results, post, settle } = await started(realOpener());
    assert.equal(await post(genuine), 202);
    await settle();
    assert.equal(results[0]?.status, "accepted-new");
    assert.equal(results[0]?.eventType, "message.delivered");
    assert.equal(results[0]?.aggregateId, JSON.parse(VECTOR.plaintext).data.messageId);
  });

  test("a tampered delivery is refused", async () => {
    const { results, post } = await started(realOpener());
    const tampered = Buffer.from(genuine);
    tampered[tampered.length - 1] = (tampered[tampered.length - 1] ?? 0) ^ 1;
    assert.equal(await post(tampered), 401);
    assert.equal(results[0]?.status, "rejected-unverified");
  });

  test("a delivery for another audience is refused", async () => {
    const { post } = await started(realOpener({ audience: "someone-else" }));
    assert.equal(await post(genuine), 401);
  });

  test("a delivery made for another app is refused though everything else verifies", async () => {
    const { results, post } = await started(realOpener({ appId: "cappsomeoneelse0000000" }));
    assert.equal(await post(genuine), 401);
    assert.equal(results[0]?.status, "rejected-unverified");
  });

  test("a delivery made through another endpoint is refused though the app and the audience match", async () => {
    const { results, post } = await started(realOpener({ endpointId: "cendpointsomeoneelse000" }));
    assert.equal(await post(genuine), 401);
    assert.equal(results[0]?.status, "rejected-unverified");
    const matching = await started(realOpener({ endpointId: VECTOR.endpointId }));
    assert.equal(await matching.post(genuine), 202);
  });

  test("a delivery whose timestamp is outside the window is refused", async () => {
    const { post } = await started(realOpener({ nowSeconds: VECTOR.iat + 3600 }));
    assert.equal(await post(genuine), 401);
  });

  test("something that is not a COSE message is refused", async () => {
    const { post } = await started(realOpener());
    assert.equal(await post(Buffer.from('{"id":"forged"}')), 401);
  });
});

function event(id: string, type: string, messageId: string, occurredAt: string): WebhookEvent {
  return {
    id,
    eventId: `source-${id}`,
    type,
    occurredAt,
    appId: "capp-example",
    endpointId: "cendpoint-example",
    data: { messageId },
  };
}

/** Hands the handler the next scripted event, whatever the body. */
function scripted(script: Array<WebhookEvent | SdkError>): EventOpener {
  const queue = [...script];
  return {
    async open() {
      const next = queue.shift();
      if (next === undefined) throw new Error("the script ran out");
      if (next instanceof SdkError) throw next;
      return next;
    },
  };
}

describe("dedupe and ordering, on events the test chooses", () => {
  test("a retry carries the same id and is acked but not reprocessed", async () => {
    const delivered = event("att-1", "message.delivered", "msg-1", "2026-10-27T10:00:00Z");
    const { results, post, settle } = await started(scripted([delivered, delivered]));
    assert.equal(await post(Buffer.from("a")), 202);
    await settle();
    assert.equal(await post(Buffer.from("b")), 202);
    await settle();
    assert.deepEqual(
      results.map((r) => r.status),
      ["accepted-new", "accepted-duplicate"],
    );
  });

  test("delivered before submitted: the late submitted is accepted but does not regress the state", async () => {
    const { receiver, results, post, settle } = await started(
      scripted([
        event("att-2", "message.delivered", "msg-2", "2026-10-27T10:00:02Z"),
        event("att-3", "message.submitted", "msg-2", "2026-10-27T10:00:01Z"),
      ]),
    );
    assert.equal(await post(Buffer.from("a")), 202);
    await settle();
    assert.equal(await post(Buffer.from("b")), 202);
    await settle();
    assert.deepEqual(
      results.map((r) => r.status),
      ["accepted-new", "accepted-out-of-order-ignored"],
    );
    assert.equal(receiver.store.get("msg-2")?.eventType, "message.delivered");
  });

  test("a delivery the opener refuses (a forgery, a replay) is a 401 and never processed", async () => {
    const { results, post, settle } = await started(
      scripted([new SdkError("unverified", { kind: "unverified" })]),
    );
    assert.equal(await post(Buffer.from("a")), 401);
    await settle();
    assert.deepEqual(
      results.map((r) => r.status),
      ["rejected-unverified"],
    );
  });

  test("a verified body that is not the envelope is a 400, and a failing backend a 500", async () => {
    const { results, post } = await started(
      scripted([
        new SdkError("not an envelope", { kind: "decode" }),
        new SdkError("store down", { kind: "backend" }),
      ]),
    );
    assert.equal(await post(Buffer.from("a")), 400);
    assert.equal(await post(Buffer.from("b")), 500);
    assert.deepEqual(
      results.map((r) => r.status),
      ["rejected-malformed", "failed-backend"],
    );
  });
});

describe("an hmac endpoint, with the original signature scheme", () => {
  const SECRET = "whsec_current";
  const PREV = "whsec_previous";
  const EVENT_ID = "11111111-1111-4111-8111-111111111111";

  function signed(
    options: { secret?: string; type?: string; messageId?: string; eventId?: string } = {},
  ) {
    const body = Buffer.from(
      JSON.stringify({
        id: "attempt-1",
        type: options.type ?? "message.delivered",
        occurredAt: "2026-10-27T10:00:00Z",
        data: { messageId: options.messageId ?? "msg-h1" },
      }),
      "utf8",
    );
    const eventId = options.eventId ?? EVENT_ID;
    const timestamp = String(Math.floor(Date.now() / 1000));
    const signature = `v1=${computeSignature(options.secret ?? SECRET, timestamp, eventId, body)}`;
    return { body, eventId, timestamp, signature };
  }

  async function startedHmac(opener: EventOpener = new HmacOpener(SECRET, PREV)) {
    const results: ProcessedResult[] = [];
    const receiver = createReceiver({
      port: 0,
      opener,
      simulatedWorkMs: 1,
      onProcessed: (r) => results.push(r),
    });
    const server = await receiver.listen();
    const port = (server.address() as AddressInfo).port;
    const post = async (
      delivery: ReturnType<typeof signed>,
      headers: Record<string, string> = {},
    ): Promise<number> =>
      (
        await fetch(`http://127.0.0.1:${port}${ROUTE}`, {
          method: "POST",
          headers: {
            "content-type": "application/json",
            "x-sms-event": "message.delivered",
            "x-sms-event-id": delivery.eventId,
            "x-sms-timestamp": delivery.timestamp,
            "x-sms-signature": delivery.signature,
            ...headers,
          },
          body: new Uint8Array(delivery.body),
        })
      ).status;
    const settle = () => new Promise((resolve) => setTimeout(resolve, 80));
    after(() => server.close());
    return { receiver, results, post, settle };
  }

  test("a delivery signed with the endpoint's secret is acked and processed", async () => {
    const { results, post, settle } = await startedHmac();
    assert.equal(await post(signed()), 202);
    await settle();
    assert.equal(results[0]?.status, "accepted-new");
    assert.equal(results[0]?.eventType, "message.delivered");
    assert.equal(results[0]?.aggregateId, "msg-h1");
  });

  test("a delivery signed with the previous secret verifies while the overlap window is open", async () => {
    const { results, post, settle } = await startedHmac();
    assert.equal(await post(signed({ secret: PREV })), 202);
    await settle();
    assert.equal(results[0]?.status, "accepted-new");
  });

  test("a receiver that has only the new secret refuses a delivery signed with the old one", async () => {
    const { post } = await startedHmac(new HmacOpener(SECRET));
    assert.equal(await post(signed({ secret: PREV })), 401);
  });

  test("a retry carries the same event id and is acked but not reprocessed", async () => {
    const { results, post, settle } = await startedHmac();
    assert.equal(await post(signed()), 202);
    await settle();
    assert.equal(await post(signed()), 202);
    await settle();
    assert.deepEqual(
      results.map((r) => r.status),
      ["accepted-new", "accepted-duplicate"],
    );
  });

  test("a forged signature, a tampered body and a swapped event id are refused", async () => {
    const { results, post } = await startedHmac();
    const genuine = signed();
    assert.equal(await post({ ...genuine, signature: `v1=${"0".repeat(64)}` }), 401);
    assert.equal(await post({ ...genuine, body: Buffer.from(`${genuine.body}x`) }), 401);
    assert.equal(await post(genuine, { "x-sms-event-id": "someone-elses-event" }), 401);
    assert.deepEqual(
      results.map((r) => r.status),
      ["rejected-unverified", "rejected-unverified", "rejected-unverified"],
    );
  });

  test("a missing signature header is refused", async () => {
    const { post } = await startedHmac();
    assert.equal(await post(signed(), { "x-sms-signature": "" }), 401);
  });

  test("a sealed delivery is not an hmac one", async () => {
    const { post } = await startedHmac();
    const sealed = Buffer.from(VECTOR.sealedHex, "hex");
    const delivery = { body: sealed, eventId: EVENT_ID, timestamp: "1", signature: "" };
    assert.equal(await post(delivery), 401);
  });

  test("an hmac receiver cannot be built without a secret", () => {
    assert.throws(() => new HmacOpener(""), /needs the endpoint's secret/);
  });
});
