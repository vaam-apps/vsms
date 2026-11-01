// `lookupDeliveryStatus` on the wire (#472): where it POSTs, what the body
// holds, how a nullable column comes back, and what each refusal becomes.
// The gateway is faked at the transport (`configureGatewayFetch`), the way
// `webhooks.test.ts` does it, so the request that is asserted is the request
// the sealing fetch would have been handed.

import { afterEach, describe, expect, it, vi } from "vitest";
import { lookupDeliveryStatus } from "./delivery-status";
import { GatewayError } from "./errors";
import { configureGatewayFetch, type GatewayFetch } from "./upstream";

function json(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });
}

const STATUS = {
  messageId: "cmsg0000000000000000001",
  appId: "capp0000000000000000001",
  recipient: "+237*******56",
  operator: "orange",
  senderIdValue: "VAAM",
  class: "otp",
  encoding: "gsm7",
  segments: 1,
  state: "delivered",
  attempts: 1,
  acceptedAt: "2026-10-30T10:00:00.000Z",
  scheduledAt: null,
  submittedAt: "2026-10-30T10:00:01.000Z",
  finalizedAt: "2026-10-30T10:00:05.000Z",
  expiresAt: "2026-10-31T10:00:00.000Z",
  purged: false,
  lastReceipt: null,
};

afterEach(() => {
  configureGatewayFetch(undefined);
  vi.restoreAllMocks();
});

describe("lookupDeliveryStatus", () => {
  it("POSTs the arguments to the procedure, without a key the caller did not set", async () => {
    const fetcher = vi.fn<GatewayFetch>(async () => json(200, { statuses: [], truncated: false }));
    configureGatewayFetch(fetcher);

    await lookupDeliveryStatus({
      messageId: "cmsg0000000000000000001",
      appId: undefined,
      since: undefined,
    });

    expect(fetcher).toHaveBeenCalledTimes(1);
    const [url, init] = fetcher.mock.calls[0] as [string, RequestInit];
    expect(new URL(url).pathname).toBe("/$procs/lookupDeliveryStatus");
    expect(init.method).toBe("POST");
    expect(init.body).toBe('{"args":{"messageId":"cmsg0000000000000000001"}}');
  });

  it("answers a nullable column as an absent field, not as null", async () => {
    configureGatewayFetch(async () => json(200, { statuses: [STATUS], truncated: false }));

    const result = await lookupDeliveryStatus({ messageId: STATUS.messageId });

    const [status] = result.statuses;
    expect(status?.scheduledAt).toBeUndefined();
    expect(status?.lastReceipt).toBeUndefined();
    expect(status?.recipient).toBe("+237*******56");
    expect(result.truncated).toBe(false);
  });

  it("answers a purged row without its recipient", async () => {
    configureGatewayFetch(async () =>
      json(200, { statuses: [{ ...STATUS, recipient: null, purged: true }], truncated: true }),
    );

    const result = await lookupDeliveryStatus({ messageId: STATUS.messageId });

    expect(result.statuses[0]?.recipient).toBeUndefined();
    expect(result.statuses[0]?.purged).toBe(true);
    expect(result.truncated).toBe(true);
  });

  it("keeps the in_flight outcome of a receipt", async () => {
    configureGatewayFetch(async () =>
      json(200, {
        statuses: [
          {
            ...STATUS,
            lastReceipt: {
              id: "crcpt",
              providerId: "cprov",
              outcome: "in_flight",
              rawStatus: "DeliveredToNetwork",
              errorCode: null,
              networkCode: "orange",
              receivedAt: "2026-10-30T10:00:03.000Z",
              occurredAt: null,
            },
          },
        ],
        truncated: false,
      }),
    );

    const result = await lookupDeliveryStatus({ messageId: STATUS.messageId });

    expect(result.statuses[0]?.lastReceipt?.outcome).toBe("in_flight");
    expect(result.statuses[0]?.lastReceipt?.errorCode).toBeUndefined();
  });

  it("a 403 keeps the status, the FORBIDDEN code and the server's text verbatim", async () => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    const text = "delivery lookup budget exhausted: 60 an hour";
    configureGatewayFetch(async () => json(403, { code: "FORBIDDEN", message: text }));

    const error = await lookupDeliveryStatus({ msisdn: "+237677123456" }).catch((e) => e);

    expect(error).toBeInstanceOf(GatewayError);
    expect(error.httpStatus).toBe(403);
    expect(error.trpcCode).toBe("FORBIDDEN");
    expect(error.message).toBe(text);
  });

  it("a 422 is BAD_REQUEST with the server's text", async () => {
    const text = "msisdn is not a valid mobile number";
    configureGatewayFetch(async () => json(422, { code: "VALIDATION_ERROR", message: text }));

    const error = await lookupDeliveryStatus({ msisdn: "12" }).catch((e) => e);

    expect(error).toBeInstanceOf(GatewayError);
    expect(error.trpcCode).toBe("BAD_REQUEST");
    expect(error.message).toBe(text);
  });
});
