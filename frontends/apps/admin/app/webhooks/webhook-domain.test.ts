import { describe, expect, it } from "vitest";
import { maskSecret, payloadFor } from "./webhook-domain";

describe("maskSecret", () => {
  it("keeps the last four characters visible and masks the rest", () => {
    expect(maskSecret("whsec_abcdef1234567890abcd")).toBe(`whsec_${"•".repeat(10)}abcd`);
  });

  it("does not throw on a value shorter than the tail length — the whole value is 'the tail'", () => {
    expect(maskSecret("ab")).toBe(`whsec_${"•".repeat(10)}ab`);
  });
});

describe("payloadFor", () => {
  it("pretty-prints a JSON payload", () => {
    const result = payloadFor({ payload: '{"a":1,"b":"two"}' });
    expect(result).toBe(JSON.stringify({ a: 1, b: "two" }, null, 2));
  });

  it("falls back to the raw string on malformed JSON, rather than throwing", () => {
    expect(payloadFor({ payload: "not json" })).toBe("not json");
  });
});
