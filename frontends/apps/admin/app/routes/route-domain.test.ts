import { describe, expect, it } from "vitest";
import { predicateSummary } from "./route-domain";

describe("predicateSummary", () => {
  it("reports 'matches anything' when every predicate is a wildcard", () => {
    expect(predicateSummary({})).toBe("matches anything");
  });

  it("lists a single set predicate", () => {
    expect(predicateSummary({ matchOperator: "orange" })).toBe("operator=orange");
  });

  it("app-scoping is reported without leaking the raw app id", () => {
    expect(predicateSummary({ matchAppId: "cs_some_app_id" })).toBe("app-scoped");
  });

  it("joins every set predicate, in field order, with the class predicate included", () => {
    expect(
      predicateSummary({
        matchOperator: "mtn",
        matchClass: "otp",
        matchAppId: "cs_app",
        matchPrefix: "677",
      }),
    ).toBe("operator=mtn, class=otp, app-scoped, prefix=677");
  });

  it("a predicate explicitly set to undefined is a wildcard, same as omitting it", () => {
    expect(
      predicateSummary({
        matchOperator: undefined,
        matchClass: "marketing",
        matchAppId: undefined,
        matchPrefix: undefined,
      }),
    ).toBe("class=marketing");
  });
});
