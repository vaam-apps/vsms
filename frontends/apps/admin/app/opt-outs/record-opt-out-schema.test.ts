import { describe, expect, it } from "vitest";
import { recordOptOutSchema } from "./record-opt-out-schema";

function baseValues(overrides: Partial<Record<string, unknown>> = {}) {
  return {
    msisdn: "+237677123456",
    source: "admin",
    scope: "all",
    reason: "",
    ...overrides,
  };
}

describe("recordOptOutSchema", () => {
  it("accepts a valid record with no reason", () => {
    const result = recordOptOutSchema.safeParse(baseValues());
    expect(result.success).toBe(true);
  });

  it("trims MSISDN and scope before validating", () => {
    const result = recordOptOutSchema.safeParse(
      baseValues({ msisdn: "  +237677123456  ", scope: "  all  " }),
    );
    expect(result.success).toBe(true);
    if (result.success) {
      expect(result.data.msisdn).toBe("+237677123456");
      expect(result.data.scope).toBe("all");
    }
  });

  it("rejects an empty (or whitespace-only) MSISDN", () => {
    expect(recordOptOutSchema.safeParse(baseValues({ msisdn: "" })).success).toBe(false);
    expect(recordOptOutSchema.safeParse(baseValues({ msisdn: "   " })).success).toBe(false);
  });

  it("rejects an empty (or whitespace-only) scope", () => {
    expect(recordOptOutSchema.safeParse(baseValues({ scope: "" })).success).toBe(false);
    expect(recordOptOutSchema.safeParse(baseValues({ scope: "   " })).success).toBe(false);
  });

  it("rejects a source outside the known set", () => {
    expect(recordOptOutSchema.safeParse(baseValues({ source: "carrier_pigeon" })).success).toBe(
      false,
    );
  });

  it("accepts every known source", () => {
    for (const source of ["inbound_stop", "admin", "import", "operator"]) {
      expect(recordOptOutSchema.safeParse(baseValues({ source })).success).toBe(true);
    }
  });
});
