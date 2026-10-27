import assert from "node:assert/strict";
import { describe, test } from "node:test";
import { openerFromEnv } from "./config.ts";

const KEYS = {
  WEBHOOK_RECEIVER_SERVER_CURRENT: "11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo",
  WEBHOOK_RECEIVER_SERVER_NEXT: "PUAXw-hDiVqStwqnTRt-vJyYLM8uxJaMwM1V8Sr0Zgw",
};

describe("openerFromEnv", () => {
  test("builds an opener from the pinned keys and the app id", () => {
    const opener = openerFromEnv({ ...KEYS, WEBHOOK_RECEIVER_APP_ID: "capp00000000000000000" });
    assert.equal(typeof opener.open, "function");
  });

  test("refuses to start without the app id: there is no mode that accepts any app", () => {
    assert.throws(() => openerFromEnv({ ...KEYS }), /WEBHOOK_RECEIVER_APP_ID is required/);
    assert.throws(
      () => openerFromEnv({ ...KEYS, WEBHOOK_RECEIVER_APP_ID: "" }),
      /WEBHOOK_RECEIVER_APP_ID is required/,
    );
  });

  test("refuses to start without the gateway's keys", () => {
    assert.throws(
      () => openerFromEnv({ WEBHOOK_RECEIVER_APP_ID: "capp00000000000000000" }),
      /WEBHOOK_RECEIVER_SERVER_CURRENT is required/,
    );
  });

  test("an empty endpoint id is no endpoint id, a set one is passed on", () => {
    const base = { ...KEYS, WEBHOOK_RECEIVER_APP_ID: "capp00000000000000000" };
    assert.ok(openerFromEnv({ ...base, WEBHOOK_RECEIVER_ENDPOINT_ID: "" }));
    assert.ok(openerFromEnv({ ...base, WEBHOOK_RECEIVER_ENDPOINT_ID: "cendpoint0000000000000" }));
  });
});
