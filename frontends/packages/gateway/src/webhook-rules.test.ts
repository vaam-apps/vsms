import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import {
  GATEWAY_AUDIENCE,
  GATEWAY_AUDIENCE_MESSAGE,
  receiverRoute,
  urlRefusal,
} from "./webhook-rules";

// One table, two languages: `sms-webhook`'s `delivery_route` (Rust, what the
// worker applies at delivery) reads the same file in
// `tests/delivery_route_cases.rs`, so a URL cannot be fine here and refused
// there, which would burn a delivery's attempts instead of a form's error.
const table = JSON.parse(
  readFileSync(
    new URL(
      "../../../../backends/crates/sms-webhook/tests/fixtures/delivery_route_cases.json",
      import.meta.url,
    ),
    "utf8",
  ),
) as { cases: Array<{ url: string; route: string | null }> };

describe("receiverRoute", () => {
  it("gives every URL in the shared table the route sms-webhook gives it", () => {
    expect(table.cases.length).toBeGreaterThan(20);
    for (const { url, route } of table.cases) {
      expect(receiverRoute(url), url).toBe(route);
    }
  });

  it("looks for ? and # in the text: the URL API drops a bare marker, the Rust parser does not", () => {
    // `new URL("https://h/a?").search` is "" and so is `.hash` of `https://h/a#`.
    expect(new URL("https://h/a?").search).toBe("");
    expect(new URL("https://h/a#").hash).toBe("");
    expect(receiverRoute("https://h/a?")).toBeNull();
    expect(receiverRoute("https://h/a#")).toBeNull();
  });
});

describe("urlRefusal and the gateway's own audience", () => {
  it("names a refusal for a URL a delivery cannot be sealed for, and none for one it can", () => {
    expect(urlRefusal("https://api.vaam.store/webhooks/vsms")).toBeNull();
    expect(urlRefusal("https://api.vaam.store/webhooks/vsms?x=1")).toMatch(/query/);
  });

  it("keeps the audience vsms for the gateway", () => {
    expect(GATEWAY_AUDIENCE).toBe("vsms");
    expect(GATEWAY_AUDIENCE_MESSAGE).toContain('"vsms"');
  });
});
