import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import {
  GATEWAY_AUDIENCE,
  GATEWAY_AUDIENCE_MESSAGE,
  receiverRoute,
  SEALABLE_URL_PATTERN,
  urlCanBeSealed,
  urlMatchesSealablePattern,
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

const sealable = JSON.parse(
  readFileSync(
    new URL(
      "../../../../backends/crates/sms-webhook/tests/fixtures/sealable_url_cases.json",
      import.meta.url,
    ),
    "utf8",
  ),
) as { cases: Array<{ url: string; pattern: boolean; sealable: boolean; why: string }> };

describe("SEALABLE_URL_PATTERN", () => {
  it("is the Rust constant, character for character", () => {
    // The Rust constant is the one the migration, the schema and sms-api's tests
    // are held to; reading it from its source is what keeps this copy from
    // becoming a second rule that merely looks like it.
    const source = readFileSync(
      new URL("../../../../backends/crates/sms-webhook/src/lib.rs", import.meta.url),
      "utf8",
    );
    const found = /pub const SEALABLE_URL_PATTERN: &str = r"([^"]+)";/.exec(source);
    expect(found, "the Rust constant is a raw string literal on one line").not.toBeNull();
    expect(SEALABLE_URL_PATTERN).toBe(found?.[1]);
  });

  it("gives every URL in the shared corpus the answer the Rust pattern gives it", () => {
    expect(sealable.cases.length).toBeGreaterThan(40);
    for (const { url, pattern, why } of sealable.cases) {
      expect(urlMatchesSealablePattern(url), `${url}: ${why}`).toBe(pattern);
    }
  });

  it("and the pattern with the route gives the recorded sealability", () => {
    for (const { url, sealable: expected, why } of sealable.cases) {
      expect(urlCanBeSealed(url), `${url}: ${why}`).toBe(expected);
      expect(urlRefusal(url) === null, url).toBe(expected);
    }
  });

  it("refuses what URL parsing would quietly accept: an IDN as typed, an underscore host, port 0", () => {
    for (const url of [
      "https://bücher.example/hook",
      "https://my_host.example/hook",
      "https://example.com:0/hook",
    ]) {
      // `new URL` takes each (the first as punycode), so only the pattern refuses them.
      expect(() => new URL(url), url).not.toThrow();
      expect(receiverRoute(url), url).not.toBeNull();
      expect(urlMatchesSealablePattern(url), url).toBe(false);
      expect(urlRefusal(url), url).not.toBeNull();
    }
    expect(urlCanBeSealed("https://xn--bcher-kva.example/hook")).toBe(true);
  });

  it("is not the whole rule: the shapes it lets through and URL parsing refuses are refused too", () => {
    for (const url of ["https://[:]/hook", "https://1.2.3.999/hook", "https://x.0/hook"]) {
      expect(urlMatchesSealablePattern(url), url).toBe(true);
      expect(receiverRoute(url), url).toBeNull();
      expect(urlCanBeSealed(url), url).toBe(false);
    }
  });
});

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
