import { describe, expect, it } from "vitest";
import { actorLabel, prettyJson, primaryKeyLabel } from "./audit-log-format";

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

describe("actorLabel", () => {
  it("reads a machine actor's claims nested under claims", () => {
    expect(
      actorLabel('{"claims":{"appId":"","kind":"app","role":"system","sub":"sms-worker:jobs:w1"}}'),
    ).toBe("system · sms-worker:jobs:w1");
  });

  it("reads a human actor's top-level claims", () => {
    expect(actorLabel('{"sub":"c580795f","kind":"user","role":"owner"}')).toBe("owner · c580795f");
  });

  it("falls back to the kind when there is no role", () => {
    expect(actorLabel('{"kind":"app","sub":"client-a"}')).toBe("app · client-a");
  });

  it("shows anything it cannot read as it came", () => {
    expect(actorLabel("not json")).toBe("not json");
    expect(actorLabel("[1,2]")).toBe("[1,2]");
    expect(actorLabel('{"other":1}')).toBe('{"other":1}');
  });
});

describe("primaryKeyLabel", () => {
  it("strips a JSON string's quotes", () => {
    expect(primaryKeyLabel('"cc8ae5b"')).toBe("cc8ae5b");
  });

  it("keeps a composite or unparseable key as it came", () => {
    expect(primaryKeyLabel('{"a":1}')).toBe('{"a":1}');
    expect(primaryKeyLabel("abc")).toBe("abc");
  });
});
