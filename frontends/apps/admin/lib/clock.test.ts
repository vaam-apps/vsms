import { describe, expect, it } from "vitest";
import { CLOCK_SKEW_WARN_SECONDS, clockSkewSeconds, isSkewed, measureClockSkew } from "./clock";

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

describe("measureClockSkew", () => {
  const now = Date.parse("2026-10-27T12:00:00Z");

  it("reads the Date header of the console's own server", async () => {
    const calls: string[] = [];
    const skew = await measureClockSkew({
      now: () => now,
      fetch: (async (input: RequestInfo | URL) => {
        calls.push(String(input));
        return new Response("{}", { headers: { date: "Tue, 27 Oct 2026 11:50:00 GMT" } });
      }) as typeof fetch,
    });

    expect(skew).toBe(600);
    expect(calls).toEqual(["/api/health"]);
  });

  it("is unknown when the server cannot be asked, never a throw", async () => {
    const skew = await measureClockSkew({
      fetch: (async () => {
        throw new Error("offline");
      }) as typeof fetch,
    });

    expect(skew).toBeUndefined();
  });
});
