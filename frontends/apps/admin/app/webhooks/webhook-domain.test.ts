import { describe, expect, it } from "vitest";
import { createEndpointSchema, endpointSchema, payloadFor, receiverRoute } from "./webhook-domain";

describe("payloadFor", () => {
  it("pretty-prints a JSON payload", () => {
    const result = payloadFor({ payload: '{"a":1,"b":"two"}' });
    expect(result).toBe(JSON.stringify({ a: 1, b: "two" }, null, 2));
  });

  it("falls back to the raw string on malformed JSON, rather than throwing", () => {
    expect(payloadFor({ payload: "not json" })).toBe("not json");
  });
});

describe("receiverRoute", () => {
  it("is the path of an http(s) URL", () => {
    expect(receiverRoute("https://api.vaam.store/webhooks/vsms")).toBe("/webhooks/vsms");
    expect(receiverRoute("http://localhost:9000/webhooks")).toBe("/webhooks");
    expect(receiverRoute("https://example.com")).toBe("/");
  });

  it("refuses a URL a delivery cannot be sealed for, the way the worker does", () => {
    for (const url of [
      "https://example.com/hooks/{tenant}",
      "https://example.com/a%2Fb",
      "https://example.com/hooks?x=1",
      "https://example.com/hooks#frag",
      "https://user:pw@example.com/hooks",
      "ftp://example.com/hooks",
      "not a url",
    ]) {
      expect(receiverRoute(url), url).toBeNull();
    }
  });
});

describe("the endpoint schemas", () => {
  const base = { url: "https://example.com/hooks", maxAttempts: "8", maskRecipient: true };

  it("require an audience, and bound it as the column does", () => {
    expect(createEndpointSchema.safeParse({ ...base, appId: "a", audience: "vaam" }).success).toBe(
      true,
    );
    expect(createEndpointSchema.safeParse({ ...base, appId: "a", audience: "  " }).success).toBe(
      false,
    );
    expect(
      createEndpointSchema.safeParse({ ...base, appId: "a", audience: "x".repeat(256) }).success,
    ).toBe(false);
    expect(endpointSchema.safeParse({ ...base, active: true, audience: "vaam" }).success).toBe(
      true,
    );
    expect(endpointSchema.safeParse({ ...base, active: true, audience: "" }).success).toBe(false);
  });
});
