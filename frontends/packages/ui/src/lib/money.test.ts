import { describe, expect, it } from "vitest";
import { currencyExponent, formatMoney } from "./money";

// `en-US` throughout: the default locale is the runtime's, which differs
// between a developer's machine and CI, and a formatter test that depends
// on it is a flake waiting for a different laptop.
const L = { locale: "en-US" } as const;

/**
 * `Intl` separates a currency code from its number with U+00A0, a
 * NO-BREAK SPACE — deliberately, so "USD" and "12.50" cannot be split
 * across a line break. `formatMoney` passes that through unchanged rather
 * than normalising it, because the typography is correct and rewriting a
 * formatter's output to look more like ASCII is how a line break ends up
 * in the middle of an amount.
 *
 * The consequence is that these strings do NOT compare equal to the
 * visually identical ones you would type, which cost this suite its first
 * seven failures. Tests normalise for comparison; one test below asserts
 * the real character is there, so the behaviour is pinned rather than
 * merely worked around.
 */
const NBSP = "\u00a0";
function plain(formatted: string): string {
  return formatted.replaceAll(NBSP, " ");
}

describe("currencyExponent", () => {
  // The set every hand-written table gets wrong. Zero-decimal currencies
  // are usually remembered; three-decimal ones almost never are.
  it.each([
    ["XAF", 0],
    ["XOF", 0],
    ["JPY", 0],
    ["CLP", 0],
    ["VND", 0],
    ["USD", 2],
    ["EUR", 2],
    ["KWD", 3],
    ["BHD", 3],
    ["TND", 3],
  ])("%s has %i decimal places", (currency, expected) => {
    expect(currencyExponent(currency, "en-US")).toBe(expected);
  });

  it("falls back to 2 for a currency Intl does not know", () => {
    expect(currencyExponent("XTS", "en-US")).toBe(2);
  });
});

describe("formatMoney", () => {
  it("formats a zero-decimal currency with no fraction", () => {
    expect(plain(formatMoney(500_000, "XAF", L))).toBe("XAF 500,000");
  });

  it("shifts a two-decimal currency by two places", () => {
    expect(plain(formatMoney(1250, "USD", L))).toBe("USD 12.50");
    expect(plain(formatMoney(5, "USD", L))).toBe("USD 0.05");
    expect(plain(formatMoney(0, "USD", L))).toBe("USD 0.00");
  });

  it("shifts a three-decimal currency by three places", () => {
    expect(plain(formatMoney(1250, "KWD", L))).toBe("KWD 1.250");
  });

  it("handles negatives on both sides of the decimal point", () => {
    expect(plain(formatMoney(-5, "USD", L))).toBe("-USD 0.05");
    expect(plain(formatMoney(-500_000, "XAF", L))).toBe("-XAF 500,000");
  });

  /**
   * The bug this module exists to make unrepeatable. Dividing minor units
   * by a power of ten in floating point silently corrupts any amount past
   * 2^53 — `9007199254740993 / 100` evaluates to `90071992547409.92`,
   * losing the final cent. String surgery has no such limit.
   */
  it("is exact past Number.MAX_SAFE_INTEGER", () => {
    expect(plain(formatMoney("9007199254740993", "USD", L))).toBe("USD 90,071,992,547,409.93");
    expect(plain(formatMoney(9_007_199_254_740_993n, "USD", L))).toBe("USD 90,071,992,547,409.93");
    // The naive implementation, shown failing rather than merely asserted.
    // Written through `Number(...)` because the literals themselves cannot
    // survive being written down: `9007199254740993` parses to
    // ...992, and `90071992547409.93` to ...94.
    const naive = Number("9007199254740993") / 100;
    expect(naive.toFixed(2)).not.toBe("90071992547409.93");
    expect(naive.toFixed(2)).toBe("90071992547409.92");
  });

  it("accepts bigint and digit strings interchangeably", () => {
    expect(formatMoney(1250n, "USD", L)).toBe(formatMoney("1250", "USD", L));
    expect(formatMoney("1250", "USD", L)).toBe(formatMoney(1250, "USD", L));
  });

  it("throws on a non-integer rather than rounding it away", () => {
    expect(() => formatMoney(12.5, "USD", L)).toThrow(/integer count of minor units/);
    expect(() => formatMoney("12.50", "USD", L)).toThrow(/integer count of minor units/);
    expect(() => formatMoney("", "USD", L)).toThrow(/integer count of minor units/);
  });

  it("honours the display mode", () => {
    expect(plain(formatMoney(1250, "USD", { ...L, display: "code" }))).toBe("USD 12.50");
    expect(formatMoney(1250, "USD", { ...L, display: "symbol" })).toBe("$12.50");
    expect(formatMoney(1250, "USD", { ...L, display: "none" })).toBe("12.50");
    expect(formatMoney(500_000, "XAF", { ...L, display: "none" })).toBe("500,000");
  });

  it("can force a sign for a ledger delta", () => {
    expect(plain(formatMoney(1250, "USD", { ...L, signDisplay: "always" }))).toBe("+USD 12.50");
  });

  it("keeps Intl's no-break space between code and amount", () => {
    expect(formatMoney(1250, "USD", L)).toBe(`USD${NBSP}12.50`);
    expect(formatMoney(1250, "USD", L)).not.toBe("USD 12.50");
  });

  it("respects the locale's own grouping and decimal marks", () => {
    expect(formatMoney(123_456_789, "EUR", { locale: "de-DE", display: "none" })).toBe(
      "1.234.567,89",
    );
  });
});
