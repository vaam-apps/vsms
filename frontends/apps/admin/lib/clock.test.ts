import { describe, expect, it } from "vitest";
import { CLOCK_SKEW_WARN_SECONDS, clockSkewSeconds, isSkewed } from "./clock";

describe("clock skew", () => {
  const now = Date.parse("2026-10-27T12:00:00Z");

  it("is this computer's clock minus the server's, in seconds", () => {
    expect(clockSkewSeconds("Tue, 27 Oct 2026 11:55:00 GMT", now)).toBe(300);
    expect(clockSkewSeconds("Tue, 27 Oct 2026 12:10:00 GMT", now)).toBe(-600);
  });

  it("is unknown when the header is missing or unreadable", () => {
    expect(clockSkewSeconds(null, now)).toBeUndefined();
    expect(clockSkewSeconds("yesterday", now)).toBeUndefined();
  });

  it("is worth a warning well before the gateway's five minutes", () => {
    expect(CLOCK_SKEW_WARN_SECONDS).toBeLessThan(300);
    expect(isSkewed(CLOCK_SKEW_WARN_SECONDS)).toBe(false);
    expect(isSkewed(CLOCK_SKEW_WARN_SECONDS + 1)).toBe(true);
    expect(isSkewed(-(CLOCK_SKEW_WARN_SECONDS + 1))).toBe(true);
    expect(isSkewed(undefined)).toBe(false);
  });
});
