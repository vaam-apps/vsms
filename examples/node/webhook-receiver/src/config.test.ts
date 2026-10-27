import assert from "node:assert/strict";
import { describe, test } from "node:test";
import { modeFromEnv, openerFromEnv } from "./config.ts";

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

describe("the delivery mode", () => {
  test("is sealed unless the environment says hmac", () => {
    assert.equal(modeFromEnv({}), "sealed");
    assert.equal(modeFromEnv({ WEBHOOK_RECEIVER_MODE: "" }), "sealed");
    assert.equal(modeFromEnv({ WEBHOOK_RECEIVER_MODE: "sealed" }), "sealed");
    assert.equal(modeFromEnv({ WEBHOOK_RECEIVER_MODE: "hmac" }), "hmac");
  });

  test("a typo refuses to start rather than picking a scheme", () => {
    assert.throws(
      () => modeFromEnv({ WEBHOOK_RECEIVER_MODE: "HMAC" }),
      /must be "sealed" or "hmac"/,
    );
    assert.throws(
      () => openerFromEnv({ ...ENV, WEBHOOK_RECEIVER_MODE: "plain" }),
      /must be "sealed" or "hmac"/,
    );
  });

  test("hmac needs the endpoint's secret and nothing the seal needs", () => {
    const opener = openerFromEnv({
      WEBHOOK_RECEIVER_MODE: "hmac",
      WEBHOOK_RECEIVER_SECRET: "whsec_x",
    });
    assert.equal(typeof opener.open, "function");
    assert.throws(
      () => openerFromEnv({ WEBHOOK_RECEIVER_MODE: "hmac" }),
      /WEBHOOK_RECEIVER_SECRET is required in hmac mode/,
    );
  });

  test("hmac accepts the previous secret for a rotation's overlap window", () => {
    const opener = openerFromEnv({
      WEBHOOK_RECEIVER_MODE: "hmac",
      WEBHOOK_RECEIVER_SECRET: "whsec_new",
      WEBHOOK_RECEIVER_PREV_SECRET: "whsec_old",
    });
    assert.equal(typeof opener.open, "function");
  });
});
