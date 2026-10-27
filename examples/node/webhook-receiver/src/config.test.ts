import assert from "node:assert/strict";
import { describe, test } from "node:test";
import { openerFromEnv } from "./config.ts";

const KEYS = {
  WEBHOOK_RECEIVER_SERVER_CURRENT: "11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo",
  WEBHOOK_RECEIVER_SERVER_NEXT: "PUAXw-hDiVqStwqnTRt-vJyYLM8uxJaMwM1V8Sr0Zgw",
};

const ENV = {
  ...KEYS,
  WEBHOOK_RECEIVER_APP_ID: "capp00000000000000000",
  WEBHOOK_RECEIVER_ENDPOINT_ID: "cendpoint0000000000000",
};

describe("openerFromEnv", () => {
  test("builds an opener from the pinned keys, the app id and the endpoint id", () => {
    const opener = openerFromEnv(ENV);
    assert.equal(typeof opener.open, "function");
  });

  test("refuses to start without the app id: there is no mode that accepts any app", () => {
    assert.throws(
      () => openerFromEnv({ ...ENV, WEBHOOK_RECEIVER_APP_ID: undefined }),
      /WEBHOOK_RECEIVER_APP_ID is required/,
    );
    assert.throws(
      () => openerFromEnv({ ...ENV, WEBHOOK_RECEIVER_APP_ID: "" }),
      /WEBHOOK_RECEIVER_APP_ID is required/,
    );
  });

  test("refuses to start without the gateway's keys", () => {
    assert.throws(
      () =>
        openerFromEnv({
          ...ENV,
          WEBHOOK_RECEIVER_SERVER_CURRENT: undefined,
          WEBHOOK_RECEIVER_SERVER_NEXT: undefined,
        }),
      /WEBHOOK_RECEIVER_SERVER_CURRENT is required/,
    );
  });

  test("refuses to start without the endpoint id: the receiver serves exactly one endpoint", () => {
    assert.throws(
      () => openerFromEnv({ ...ENV, WEBHOOK_RECEIVER_ENDPOINT_ID: undefined }),
      /WEBHOOK_RECEIVER_ENDPOINT_ID is required/,
    );
    assert.throws(
      () => openerFromEnv({ ...ENV, WEBHOOK_RECEIVER_ENDPOINT_ID: "" }),
      /WEBHOOK_RECEIVER_ENDPOINT_ID is required/,
    );
  });
});
