import { describe, expect, it } from "vitest";
import { describeDelta, formatCount, formatPercent, overallDeliveryRate } from "./format";

describe("formatCount", () => {
  it("adds thousands separators", () => {
    expect(formatCount(1234567)).toBe("1,234,567");
  });

  it("formats zero plainly", () => {
    expect(formatCount(0)).toBe("0");
  });
});

describe("formatPercent", () => {
  it("rounds to a whole percent at or above 10%", () => {
    expect(formatPercent(0.5)).toBe("50%");
    expect(formatPercent(0.1)).toBe("10%");
  });

  it("keeps one decimal place below 10%, so a real but small ratio isn't rounded to 0%", () => {
    expect(formatPercent(0.05)).toBe("5.0%");
    expect(formatPercent(0.001)).toBe("0.1%");
  });

  it("formats a zero ratio as 0.0%, not 0%, matching the below-10% branch", () => {
    expect(formatPercent(0)).toBe("0.0%");
  });
});

describe("overallDeliveryRate", () => {
  it("sums delivered and terminal across operators", () => {
    expect(
      overallDeliveryRate([
        { delivered: 8, terminalTotal: 10 },
        { delivered: 1, terminalTotal: 2 },
      ]),
    ).toEqual({ ratio: 0.75, delivered: 9, terminal: 12 });
  });

  it("is null when nothing reached a final state, never a 0% that reads as an outage", () => {
    expect(overallDeliveryRate([])).toBeNull();
    expect(overallDeliveryRate([{ delivered: 0, terminalTotal: 0 }])).toBeNull();
  });
});

describe("describeDelta", () => {
  it("signs a change and says when there was none", () => {
    expect(describeDelta(8)).toBe("+8 on the previous hour");
    expect(describeDelta(-1200)).toBe("−1,200 on the previous hour");
    expect(describeDelta(0)).toBe("Level with the previous hour");
    expect(describeDelta(null)).toBeNull();
  });
});
