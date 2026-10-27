import { strict as assert } from "node:assert";
import { test } from "node:test";
import { parseServerKeysReport, SdkError } from "../dist/index.js";

test("sms-gateway server-keys' output parses into the pinned pair", () => {
  const report =
    "current: 11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo\r\nnext:   PUAXw-hDiVqStwqnTRt-vJyYLM8uxJaMwM1V8Sr0Zgw\n";
  assert.deepEqual(parseServerKeysReport(report), {
    current: "11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo",
    next: "PUAXw-hDiVqStwqnTRt-vJyYLM8uxJaMwM1V8Sr0Zgw",
  });
});

test("a report missing a line is a config error naming the line", () => {
  assert.throws(
    () => parseServerKeysReport("current: only-one\n"),
    (err: unknown) =>
      err instanceof SdkError && err.kind === "config" && err.message.includes("`next:`"),
  );
});
