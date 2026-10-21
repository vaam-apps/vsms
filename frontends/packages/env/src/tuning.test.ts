import { afterEach, describe, expect, it } from "vitest";
import { coerceTuning, withTuningCoerced } from "./tuning";

describe("coerceTuning", () => {
  it("turns the raw environment string into a number", () => {
    expect(coerceTuning("MESSAGE_STREAM_POLL_MS", "2000")).toBe(2000);
    expect(typeof coerceTuning("MESSAGE_STREAM_POLL_MS", "2000")).toBe("number");
  });

  it("falls back to the schema default when unset, blank or unusable", () => {
    expect(coerceTuning("DIAGNOSTICS_POLL_MS", undefined)).toBe(5000);
    expect(coerceTuning("DIAGNOSTICS_POLL_MS", "")).toBe(5000);
    expect(coerceTuning("DIAGNOSTICS_POLL_MS", "soon")).toBe(5000);
    expect(coerceTuning("DIAGNOSTICS_POLL_MS", "10")).toBe(5000);
  });
});

describe("withTuningCoerced", () => {
  const saved = process.env.MESSAGE_STREAM_POLL_MS;
  afterEach(() => {
    if (saved === undefined) delete process.env.MESSAGE_STREAM_POLL_MS;
    else process.env.MESSAGE_STREAM_POLL_MS = saved;
  });

  it("answers tuning keys with numbers and passes everything else through", () => {
    process.env.MESSAGE_STREAM_POLL_MS = "2000";
    // What a validation-skipped createEnv hands back: raw strings.
    const raw = { MESSAGE_STREAM_POLL_MS: "2000", SMS_API_URL: "http://gw" };
    const env = withTuningCoerced(raw) as unknown as Record<string, unknown>;
    expect(env.MESSAGE_STREAM_POLL_MS).toBe(2000);
    expect(env.SMS_API_URL).toBe("http://gw");
    // The long-poll deadline the bug turned into a string concatenation.
    expect(Date.now() + (env.MESSAGE_STREAM_POLL_MS as number) - Date.now()).toBeLessThan(3000);
  });
});
