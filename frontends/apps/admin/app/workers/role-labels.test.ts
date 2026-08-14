import { describe, expect, it } from "vitest";
import { ROLE_LABELS, roleLabel } from "./role-labels";

describe("ROLE_LABELS", () => {
  it("covers every §7.1 role", () => {
    expect(Object.keys(ROLE_LABELS).sort()).toEqual(
      ["dispatch", "drain", "hooks", "jobs", "scheduler", "smpp"].sort(),
    );
  });
});

describe("roleLabel", () => {
  it("maps a known role to its label", () => {
    expect(roleLabel("dispatch")).toBe("Dispatch");
    expect(roleLabel("smpp")).toBe("SMPP");
  });

  it("falls back to the raw role string for an unrecognised role", () => {
    // A seventh role landing on the backend before this table is updated
    // must still render something readable, not throw or disappear.
    expect(roleLabel("some_future_role")).toBe("some_future_role");
  });
});
