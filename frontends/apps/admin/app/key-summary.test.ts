import { describe, expect, it } from "vitest";
import { summarizeKey } from "./key-summary";

const NOW = Date.parse("2026-10-27T12:00:00Z");
const meta = (days: number) => ({
  keyId: "k",
  userId: "u",
  pairedAt: "2026-10-01T00:00:00Z",
  expiresAt: new Date(NOW + days * 86_400_000).toISOString(),
});

describe("summarizeKey", () => {
  it("says how long a paired key has left", () => {
    expect(summarizeKey({ kind: "paired", meta: meta(12) }, NOW)).toBe("Paired, 12 days left");
    expect(summarizeKey({ kind: "paired", meta: meta(1) }, NOW)).toBe("Paired, 1 day left");
    expect(summarizeKey({ kind: "paired", meta: meta(0.001) }, NOW)).toBe("Paired, 1 day left");
  });

  it("says what is wrong with a key that is not good", () => {
    expect(summarizeKey({ kind: "ended", cause: "expired", meta: meta(-1) }, NOW)).toBe(
      "Key expired",
    );
    expect(summarizeKey({ kind: "ended", cause: "refused", meta: meta(5) }, NOW)).toBe(
      "Key no longer accepted",
    );
    expect(summarizeKey({ kind: "none" }, NOW)).toBe("Not paired");
    expect(summarizeKey({ kind: "pending" }, NOW)).toBe("Not paired");
    expect(summarizeKey({ kind: "unreachable" }, NOW)).toBe("Gateway unreachable");
  });
});
