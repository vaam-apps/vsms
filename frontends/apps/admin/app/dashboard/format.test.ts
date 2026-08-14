import { describe, expect, it } from "vitest";
import { formatCount, formatPercent } from "./format";

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
