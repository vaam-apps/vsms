import { describe, expect, it } from "vitest";
import { looksLikeAttemptedMsisdn } from "./composer-validation";

describe("looksLikeAttemptedMsisdn", () => {
  it("is false for an empty string", () => {
    expect(looksLikeAttemptedMsisdn("")).toBe(false);
  });

  it("is false for a short fragment (below the 8-digit floor)", () => {
    expect(looksLikeAttemptedMsisdn("+237677")).toBe(false);
  });

  it("is true right at the 8-digit floor", () => {
    expect(looksLikeAttemptedMsisdn("67712345")).toBe(true);
  });

  it("is true for a complete E.164 Cameroon mobile number", () => {
    expect(looksLikeAttemptedMsisdn("+237677123456")).toBe(true);
  });

  it("counts only digits — spaces and punctuation don't contribute", () => {
    expect(looksLikeAttemptedMsisdn("+237 677")).toBe(false); // 6 digits
    expect(looksLikeAttemptedMsisdn("+237 677 12")).toBe(true); // 8 digits
  });

  it("is true for a non-Cameroon-shaped but long-enough digit run (the point: this is a fast heuristic, not validation)", () => {
    expect(looksLikeAttemptedMsisdn("00000000")).toBe(true);
  });
});
