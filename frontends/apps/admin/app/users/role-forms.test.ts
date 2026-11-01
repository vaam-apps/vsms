import { describe, expect, it } from "vitest";
import { isReservedRoleKey, isValidRoleKeyShape, KNOWN_PERMISSIONS } from "./role-forms";

describe("isReservedRoleKey", () => {
  it("flags 'system'", () => {
    expect(isReservedRoleKey("system")).toBe(true);
  });

  it("flags 'app'", () => {
    expect(isReservedRoleKey("app")).toBe(true);
  });

  it("does not flag an ordinary role key", () => {
    expect(isReservedRoleKey("operator")).toBe(false);
  });

  it("is case-sensitive — 'System' is not the reserved literal", () => {
    expect(isReservedRoleKey("System")).toBe(false);
  });
});

describe("isValidRoleKeyShape", () => {
  it("accepts a lowercase key starting with a letter", () => {
    expect(isValidRoleKeyShape("operator")).toBe(true);
    expect(isValidRoleKeyShape("billing_ops")).toBe(true);
  });

  it("rejects a key starting with a digit", () => {
    expect(isValidRoleKeyShape("1operator")).toBe(false);
  });

  it("rejects uppercase characters", () => {
    expect(isValidRoleKeyShape("Operator")).toBe(false);
  });

  it("rejects a key shorter than 3 characters", () => {
    expect(isValidRoleKeyShape("ab")).toBe(false);
  });

  it("rejects a key longer than 32 characters", () => {
    expect(isValidRoleKeyShape("a".repeat(33))).toBe(false);
  });

  it("rejects hyphens", () => {
    expect(isValidRoleKeyShape("billing-ops")).toBe(false);
  });
});

describe("KNOWN_PERMISSIONS", () => {
  it("offers both permissions that read opt-outs: optout:read (the auditor's) and optout:manage", () => {
    // `OptOut` and `ConsentRecord` read policies ask the optOutRead claim,
    // which a person's role gets from either (auth::Grants::of).
    expect(KNOWN_PERMISSIONS as readonly string[]).toContain("optout:read");
    expect(KNOWN_PERMISSIONS as readonly string[]).toContain("optout:manage");
  });
});
