// #55: `packEventTypes`/`unpackEventTypes` have to agree byte-for-byte with
// `sms_core::pack`/`unpack` (`backends/crates/sms-core/src/lib.rs`) — this is the
// wire format `dlr.rs`'s own subscriber match
// (`webhook_endpoint::eventTypes().contains(sms_core::needle(event_type))`)
// depends on. A drift here wouldn't fail loudly; it would mean an operator's
// event-type selection silently stops matching, or silently matches
// everything (an empty column read as "no filter" rather than "no events").

import { describe, expect, it } from "vitest";
import { configureGatewayFetch } from "./upstream";
import {
  type CreateWebhookEndpointFields,
  createWebhookEndpoint,
  generateWebhookSecret,
  packEventTypes,
  unpackEventTypes,
} from "./webhooks";

describe("packEventTypes", () => {
  it("wraps a single value in sentinel spaces", () => {
    expect(packEventTypes(["message.delivered"])).toBe(" message.delivered ");
  });

  it("joins multiple values with exactly one space, sentinels on both ends", () => {
    expect(packEventTypes(["message.accepted", "message.delivered"])).toBe(
      " message.accepted message.delivered ",
    );
  });

  /**
   * The actual trap this file exists to catch — see `webhooks.ts`'s own
   * doc comment on [`packEventTypes`]. `sms_core::pack(Vec::<String>::new())
   * == EMPTY == " "`, one space, not two. A naive `` ` ${arr.join(" ")} ` ``
   * implementation produces `"  "` for an empty array instead — this
   * assertion is what would have caught that regression before it shipped.
   */
  it("packs an empty selection to exactly one space, not two", () => {
    expect(packEventTypes([])).toBe(" ");
    expect(packEventTypes([]).length).toBe(1);
  });
});

describe("unpackEventTypes", () => {
  it("round-trips through packEventTypes", () => {
    const types = ["message.accepted", "message.delivered", "message.failed"] as const;
    expect(unpackEventTypes(packEventTypes(types))).toEqual(types);
  });

  it("unpacks the empty-selection sentinel to an empty array", () => {
    expect(unpackEventTypes(" ")).toEqual([]);
  });

  it("is tolerant of repeated internal whitespace, matching sms_core::unpack's split_whitespace", () => {
    expect(unpackEventTypes("  message.accepted   message.delivered  ")).toEqual([
      "message.accepted",
      "message.delivered",
    ]);
  });
});

// VS-8R: an `hmac` endpoint is created with a secret generated here, in the
// exact format `sms_webhook::generate_secret()` produces, and a `sealed` one
// with an audience and no secret. The gateway answers a mismatch with a
// database error (CHECKs in migration 0010), so what this module sends is the
// first line of defence.
describe("createWebhookEndpoint", () => {
  const fields = {
    appId: "capp00000000000000000",
    url: "https://receiver.example/webhooks/vsms",
    eventTypes: ["message.delivered" as const],
    maskRecipient: true,
    maxAttempts: 8,
  };

  async function sent(input: CreateWebhookEndpointFields) {
    const bodies: Record<string, unknown>[] = [];
    configureGatewayFetch(async (_url, init) => {
      bodies.push(JSON.parse(String(init?.body)));
      return new Response(
        JSON.stringify({
          ...bodies[0],
          id: "cendpoint",
          eventTypes: " message.delivered ",
          version: 1,
        }),
        { status: 201, headers: { "content-type": "application/json" } },
      );
    });
    const created = await createWebhookEndpoint(input);
    return { body: bodies[0] ?? {}, created };
  }

  it("sends a sealed endpoint with its audience and no secret", async () => {
    const { body, created } = await sent({ ...fields, deliveryMode: "sealed", audience: "vaam" });
    expect(body.deliveryMode).toBe("sealed");
    expect(body.audience).toBe("vaam");
    expect("secret" in body).toBe(false);
    expect(created.secret).toBeUndefined();
  });

  it("sends an hmac endpoint with a generated secret and no audience, and hands the secret back", async () => {
    const { body, created } = await sent({ ...fields, deliveryMode: "hmac" });
    expect(body.deliveryMode).toBe("hmac");
    expect("audience" in body).toBe(false);
    expect(body.secret).toMatch(/^whsec_[0-9a-f]{64}$/);
    expect(created.secret).toBe(body.secret);
  });

  it("generates a different secret every time", () => {
    expect(generateWebhookSecret()).not.toBe(generateWebhookSecret());
    expect(generateWebhookSecret()).toMatch(/^whsec_[0-9a-f]{64}$/);
  });
});
