import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it, vi } from "vitest";
import { type Focusable, focusChoice, type RadioRoot } from "./focus-choice";

function rootOf(found: Record<string, Focusable>): RadioRoot {
  return { querySelector: (selector) => found[selector] ?? null };
}
const TAB_STOP = '[role="radio"][tabindex="0"]';
const ANY = '[role="radio"]';

describe("focusChoice", () => {
  it("focuses the radio that holds the tab stop when there is one", () => {
    const stop = { focus: vi.fn() };
    const first = { focus: vi.fn() };
    expect(focusChoice(rootOf({ [TAB_STOP]: stop, [ANY]: first }))).toBe(true);
    expect(stop.focus).toHaveBeenCalledTimes(1);
    expect(first.focus).not.toHaveBeenCalled();
  });

  it("falls back to the first radio", () => {
    const first = { focus: vi.fn() };
    expect(focusChoice(rootOf({ [ANY]: first }))).toBe(true);
    expect(first.focus).toHaveBeenCalledTimes(1);
  });

  it("does nothing when the group is not on the page or holds no radio", () => {
    expect(focusChoice(null)).toBe(false);
    expect(focusChoice(rootOf({}))).toBe(false);
  });
});

describe("the screen uses it", () => {
  it("focuses the app picker when the server's error is about the app", () => {
    const screen = readFileSync(
      fileURLToPath(new URL("delivery-lookup-screen.tsx", import.meta.url)),
      "utf8",
    );
    expect(screen).toMatch(/view\.field === "appId"\) focusChoice\(appGroupRef\.current\)/);
  });
});
