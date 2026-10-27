// The static headers every route of the console is served with. A page of this
// console is a page whose key an attacker can ask to sign; framing it is how
// they would ask without the person seeing, so both framing headers are held
// here, on every route, rather than trusted to stay in a config nobody reads.

import { describe, expect, it } from "vitest";
import config from "../next.config";

describe("next.config headers", () => {
  it("forbid framing on every route, in both the old header and the new", async () => {
    const rules = (await config.headers?.()) ?? [];
    const everywhere = rules.filter((rule) => rule.source === "/:path*");
    expect(everywhere).toHaveLength(1);
    const headers = Object.fromEntries(
      (everywhere[0]?.headers ?? []).map((h) => [h.key.toLowerCase(), h.value]),
    );

    expect(headers["x-frame-options"]).toBe("DENY");
    expect(headers["content-security-policy"]).toBe("frame-ancestors 'none'");
  });

  it("keep the hardening that was already there", async () => {
    const rules = (await config.headers?.()) ?? [];
    const headers = Object.fromEntries(
      (rules[0]?.headers ?? []).map((h) => [h.key.toLowerCase(), h.value]),
    );

    expect(headers["x-content-type-options"]).toBe("nosniff");
    expect(headers["referrer-policy"]).toBe("no-referrer");
    expect(headers["cross-origin-opener-policy"]).toBe("same-origin");
  });
});
