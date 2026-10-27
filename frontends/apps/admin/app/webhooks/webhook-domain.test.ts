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
      // A bare marker is still a query or a fragment to the worker.
      "https://example.com/hooks?",
      "https://example.com/hooks#",
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

describe("what the endpoint schemas refuse on write", () => {
  const base = { maxAttempts: "8", maskRecipient: true, audience: "vaam" };

  it("refuse a URL a delivery cannot be sealed for, as the gateway does", () => {
    for (const url of [
      "https://example.com/hooks?x=1",
      "https://example.com/hooks?",
      "https://example.com/hooks#",
      "https://user:pw@example.com/hooks",
      "https://example.com/hooks/{tenant}",
      "ftp://example.com/hooks",
    ]) {
      expect(createEndpointSchema.safeParse({ ...base, appId: "a", url }).success, url).toBe(false);
      expect(endpointSchema.safeParse({ ...base, active: true, url }).success, url).toBe(false);
    }
    const ok = "https://example.com/webhooks/vsms";
    expect(createEndpointSchema.safeParse({ ...base, appId: "a", url: ok }).success).toBe(true);
    expect(endpointSchema.safeParse({ ...base, active: true, url: ok }).success).toBe(true);
  });

  it("refuse the audience vsms, the gateway's own name, and no other", () => {
    const url = "https://example.com/webhooks/vsms";
    for (const audience of ["vsms", "  vsms  "]) {
      expect(createEndpointSchema.safeParse({ ...base, appId: "a", url, audience }).success).toBe(
        false,
      );
      expect(endpointSchema.safeParse({ ...base, active: true, url, audience }).success).toBe(
        false,
      );
    }
    for (const audience of ["vsms-receiver", "Vsms", "https://example.com"]) {
      expect(createEndpointSchema.safeParse({ ...base, appId: "a", url, audience }).success).toBe(
        true,
      );
    }
  });
});
