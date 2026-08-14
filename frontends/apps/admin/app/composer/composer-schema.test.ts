import { describe, expect, it } from "vitest";
import {
  COMPOSER_FIELDS,
  composerSchema,
  DEFAULT_VALUES,
  isComposerField,
} from "./composer-schema";

function values(overrides: Partial<Record<string, unknown>> = {}) {
  return {
    to: "+237677123456",
    body: "Hello",
    senderId: "ACME",
    class: "transactional",
    clientRef: "",
    scheduledAt: "",
    validityMinutes: "",
    ...overrides,
  };
}

describe("composerSchema", () => {
  it("accepts a minimal valid submission", () => {
    const result = composerSchema.safeParse(values());
    expect(result.success).toBe(true);
  });

  it("requires a non-empty recipient", () => {
    const result = composerSchema.safeParse(values({ to: "" }));
    expect(result.success).toBe(false);
  });

  it("rejects a recipient with letters", () => {
    const result = composerSchema.safeParse(values({ to: "call-me" }));
    expect(result.success).toBe(false);
  });

  it("requires a non-empty body", () => {
    const result = composerSchema.safeParse(values({ body: "  " }));
    expect(result.success).toBe(false);
  });

  it("rejects a sender id shorter than 3 characters", () => {
    const result = composerSchema.safeParse(values({ senderId: "AB" }));
    expect(result.success).toBe(false);
  });

  it("accepts an empty sender id (falls back to the app default)", () => {
    const result = composerSchema.safeParse(values({ senderId: "" }));
    expect(result.success).toBe(true);
  });

  it("rejects a sender id longer than 11 characters", () => {
    const result = composerSchema.safeParse(values({ senderId: "TWELVE-CHARS" }));
    expect(result.success).toBe(false);
  });

  it("rejects an unknown message class", () => {
    const result = composerSchema.safeParse(values({ class: "bulk" }));
    expect(result.success).toBe(false);
  });

  it("rejects validityMinutes that isn't whole digits", () => {
    const result = composerSchema.safeParse(values({ validityMinutes: "3.5" }));
    expect(result.success).toBe(false);
  });

  it("accepts an empty validityMinutes (class default applies)", () => {
    const result = composerSchema.safeParse(values({ validityMinutes: "" }));
    expect(result.success).toBe(true);
  });
});

describe("DEFAULT_VALUES", () => {
  // `DEFAULT_VALUES` is the untouched-form state (react-hook-form's
  // `defaultValues`), not a pre-validated submission — `to`/`body` are
  // required fields, so an empty form is expected to fail validation
  // until the operator fills them in. This only asserts the shape is
  // otherwise well-formed: every key `composerSchema` knows about is
  // present, with no unexpected extra fields.
  it("declares exactly the fields composerSchema's own shape expects", () => {
    expect(Object.keys(DEFAULT_VALUES).sort()).toEqual(Object.keys(composerSchema.shape).sort());
  });
});

describe("isComposerField", () => {
  it("accepts every declared composer field", () => {
    for (const field of COMPOSER_FIELDS) {
      expect(isComposerField(field)).toBe(true);
    }
  });

  it("rejects a field the server didn't name", () => {
    expect(isComposerField("someUnrelatedField")).toBe(false);
  });
});
