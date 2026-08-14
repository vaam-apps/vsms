import { describe, expect, it } from "vitest";
import { parseIpAllowlistLines, toIpAllowlistLines } from "./ip-allowlist";

describe("toIpAllowlistLines", () => {
  it("joins entries with newlines", () => {
    expect(toIpAllowlistLines(["10.0.0.0/8", "192.168.1.1"])).toBe("10.0.0.0/8\n192.168.1.1");
  });

  it("returns an empty string for no entries", () => {
    expect(toIpAllowlistLines([])).toBe("");
  });
});

describe("parseIpAllowlistLines", () => {
  it("splits on newlines and trims whitespace", () => {
    expect(parseIpAllowlistLines(" 10.0.0.0/8 \n192.168.1.1\n")).toEqual([
      "10.0.0.0/8",
      "192.168.1.1",
    ]);
  });

  it("also splits on commas", () => {
    expect(parseIpAllowlistLines("10.0.0.0/8, 192.168.1.1")).toEqual(["10.0.0.0/8", "192.168.1.1"]);
  });

  it("drops blank lines", () => {
    expect(parseIpAllowlistLines("10.0.0.0/8\n\n\n192.168.1.1")).toEqual([
      "10.0.0.0/8",
      "192.168.1.1",
    ]);
  });

  it("returns an empty array for blank input", () => {
    expect(parseIpAllowlistLines("")).toEqual([]);
    expect(parseIpAllowlistLines("   \n  ")).toEqual([]);
  });

  it("round-trips through toIpAllowlistLines", () => {
    const entries = ["10.0.0.0/8", "192.168.1.1"];
    expect(parseIpAllowlistLines(toIpAllowlistLines(entries))).toEqual(entries);
  });
});
