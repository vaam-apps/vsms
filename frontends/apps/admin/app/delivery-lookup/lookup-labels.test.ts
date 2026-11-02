import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import {
  appNameIndex,
  BUDGET_NOTE,
  emptyCaveat,
  KIND_OPTIONS,
  outcomeLabel,
  resultsSummary,
  truncatedNote,
} from "./lookup-labels";
import { LOOKUP_KINDS } from "./lookup-query";

describe("the key options", () => {
  it("offer each kind exactly once, in the URL's vocabulary", () => {
    expect(KIND_OPTIONS.map((o) => o.value)).toEqual([...LOOKUP_KINDS]);
    expect(KIND_OPTIONS.map((o) => o.label)).toEqual([
      "Message id",
      "Client reference",
      "Phone number",
    ]);
  });
});

describe("outcomeLabel", () => {
  it("says an in-flight receipt is progress, not a verdict", () => {
    expect(outcomeLabel("in_flight")).toBe("In flight (progress, not an outcome)");
  });

  it("labels the verdicts", () => {
    expect(outcomeLabel("delivered")).toBe("Delivered");
    expect(outcomeLabel("unknown")).toBe("Unknown");
  });

  it("covers every outcome the schema declares", () => {
    const schema = readFileSync(
      new URL("../../../../../schemas/vsms.cstack", import.meta.url),
      "utf8",
    );
    const block = /enum DeliveryOutcome \{([\s\S]*?)\n\}/.exec(schema)?.[1] ?? "";
    const outcomes = block
      .split("\n")
      .map((line) => line.trim())
      .filter((line) => line !== "" && !line.startsWith("//"));
    expect(outcomes.length).toBeGreaterThanOrEqual(7);
    for (const outcome of outcomes) {
      expect(outcomeLabel(outcome as Parameters<typeof outcomeLabel>[0]), outcome).toBeTruthy();
    }
  });
});

describe("the summary and notes", () => {
  it("counts without quoting anything typed", () => {
    expect(resultsSummary(0)).toBe("No message matched");
    expect(resultsSummary(1)).toBe("1 message found");
    expect(resultsSummary(2)).toBe("2 messages found");
  });

  it("says how many of a truncated answer are shown", () => {
    expect(truncatedNote(10)).toBe(
      "Showing the newest 10. More messages matched: narrow the window.",
    );
  });

  it("gives each kind its own caveat for an empty answer", () => {
    expect(new Set(LOOKUP_KINDS.map(emptyCaveat)).size).toBe(LOOKUP_KINDS.length);
    expect(emptyCaveat("number")).toContain("pepper");
    expect(emptyCaveat("ref")).toContain("90 days");
  });

  it("the budget note states the permission, the 60 an hour and the audit", () => {
    expect(BUDGET_NOTE).toContain("delivery:read");
    expect(BUDGET_NOTE).toContain("60 an hour");
    expect(BUDGET_NOTE).toContain("recorded against your account");
  });
});

describe("appNameIndex", () => {
  it("looks an app's name up by id, and has nothing for an app it was not given", () => {
    const index = appNameIndex([
      { id: "capp1", name: "Mobile money" },
      { id: "capp2", name: "Shop" },
    ]);
    expect(index.capp2).toBe("Shop");
    expect(index.capp3).toBeUndefined();
    expect(appNameIndex([])).toEqual({});
  });
});
