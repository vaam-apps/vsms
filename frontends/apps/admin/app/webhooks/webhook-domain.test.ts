import { describe, expect, it } from "vitest";
import { payloadFor } from "./webhook-domain";

describe("payloadFor", () => {
  it("pretty-prints a JSON payload", () => {
    const result = payloadFor({ payload: '{"a":1,"b":"two"}' });
    expect(result).toBe(JSON.stringify({ a: 1, b: "two" }, null, 2));
  });

  it("falls back to the raw string on malformed JSON, rather than throwing", () => {
    expect(payloadFor({ payload: "not json" })).toBe("not json");
  });
});
