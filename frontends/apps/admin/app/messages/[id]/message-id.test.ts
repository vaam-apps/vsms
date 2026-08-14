import { describe, expect, it } from "vitest";
import { isValidMessageId } from "./message-id";

describe("isValidMessageId", () => {
  it("accepts a real cs_cuid() id (23 lowercase alphanumeric chars)", () => {
    expect(isValidMessageId("c555dbfe772717725082f0c")).toBe(true);
  });

  it("accepts the shortest allowed id (2 chars)", () => {
    expect(isValidMessageId("ab")).toBe(true);
  });

  it("accepts the longest allowed id (32 chars)", () => {
    expect(isValidMessageId("a".repeat(32))).toBe(true);
  });

  it("rejects an id one char below the minimum", () => {
    expect(isValidMessageId("a")).toBe(false);
  });

  it("rejects an id one char above the maximum", () => {
    expect(isValidMessageId("a".repeat(33))).toBe(false);
  });

  it("rejects uppercase characters", () => {
    expect(isValidMessageId("C555dbfe772717725082f0c")).toBe(false);
  });

  it("rejects a prefixed id (e.g. a msg_ prefix)", () => {
    expect(isValidMessageId("msg_c555dbfe77271772")).toBe(false);
  });

  it("rejects punctuation and whitespace", () => {
    expect(isValidMessageId("../etc/passwd")).toBe(false);
    expect(isValidMessageId("has space")).toBe(false);
    expect(isValidMessageId("")).toBe(false);
  });
});
