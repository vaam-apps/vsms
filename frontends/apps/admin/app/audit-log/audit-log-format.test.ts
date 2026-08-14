import { describe, expect, it } from "vitest";
import { prettyJson } from "./audit-log-format";

describe("prettyJson", () => {
  it("returns undefined for an undefined input", () => {
    expect(prettyJson(undefined)).toBeUndefined();
  });

  it("pretty-prints a JSON object with two-space indentation", () => {
    expect(prettyJson('{"a":1,"b":"two"}')).toBe('{\n  "a": 1,\n  "b": "two"\n}');
  });

  it("falls back to the raw string when it does not parse as JSON", () => {
    expect(prettyJson("not json at all")).toBe("not json at all");
  });

  it("falls back to the raw string for an empty string", () => {
    expect(prettyJson("")).toBe("");
  });
});
