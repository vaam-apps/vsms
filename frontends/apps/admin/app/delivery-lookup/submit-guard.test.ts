import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it, vi } from "vitest";
import { type InFlight, ignoreHeldEnter, runOnce } from "./submit-guard";

describe("runOnce", () => {
  it("two calls before anything settles run exactly once", () => {
    const flag: InFlight = { current: false };
    const mutate = vi.fn();
    expect(runOnce(flag, mutate)).toBe(true);
    expect(runOnce(flag, mutate)).toBe(false);
    expect(mutate).toHaveBeenCalledTimes(1);
  });

  it("a held key's whole burst is one run", () => {
    const flag: InFlight = { current: false };
    const mutate = vi.fn();
    for (let i = 0; i < 25; i += 1) runOnce(flag, mutate);
    expect(mutate).toHaveBeenCalledTimes(1);
  });

  it("runs again once the settled lookup has cleared the flag", () => {
    const flag: InFlight = { current: false };
    const mutate = vi.fn();
    runOnce(flag, mutate);
    flag.current = false; // the mutation's onSettled
    runOnce(flag, mutate);
    expect(mutate).toHaveBeenCalledTimes(2);
  });

  it("sets the flag before it runs, so a re-entrant call is refused", () => {
    const flag: InFlight = { current: false };
    const inner = vi.fn();
    runOnce(flag, () => {
      expect(flag.current).toBe(true);
      runOnce(flag, inner);
    });
    expect(inner).not.toHaveBeenCalled();
  });

  it("clears the flag when run throws, so the form is not left dead", () => {
    const flag: InFlight = { current: false };
    expect(() =>
      runOnce(flag, () => {
        throw new Error("boom");
      }),
    ).toThrow("boom");
    expect(flag.current).toBe(false);
    const mutate = vi.fn();
    runOnce(flag, mutate);
    expect(mutate).toHaveBeenCalledTimes(1);
  });
});

describe("ignoreHeldEnter", () => {
  const press = (key: string, repeat: boolean) => ({ key, repeat, preventDefault: vi.fn() });

  it("stops an Enter that is repeating", () => {
    const event = press("Enter", true);
    ignoreHeldEnter(event);
    expect(event.preventDefault).toHaveBeenCalledTimes(1);
  });

  it("lets the first Enter through", () => {
    const event = press("Enter", false);
    ignoreHeldEnter(event);
    expect(event.preventDefault).not.toHaveBeenCalled();
  });

  it("leaves a repeating letter alone: typing 'aaa' still works", () => {
    const event = press("a", true);
    ignoreHeldEnter(event);
    expect(event.preventDefault).not.toHaveBeenCalled();
  });
});

// The helper is only a guard if the screen goes through it. A source scan, as
// `delivery-lookup-url-state.test.ts` does: the screen is hook-driven and this
// app's tests have no DOM to mount it in.
describe("the screen is wired to the guard", () => {
  const HERE = fileURLToPath(new URL(".", import.meta.url));
  const code = (file: string) =>
    readFileSync(`${HERE}${file}`, "utf8")
      .replace(/\/\*[\s\S]*?\*\//g, "")
      .replace(/^\s*\/\/.*$/gm, "");
  const screen = code("delivery-lookup-screen.tsx");
  const form = code("components/lookup-form.tsx");

  it("holds the flag in a ref, not in state", () => {
    expect(screen).toMatch(/const inFlight = useRef\(false\)/);
    expect(screen).not.toMatch(/useState/);
  });

  it("sends only through runOnce", () => {
    expect(screen).toMatch(/runOnce\(inFlight,\s*\(\) =>\s*\{[\s\S]*?lookup\.mutate\(/);
    expect(screen.match(/\.mutate\(/g)).toHaveLength(1);
  });

  it("clears the flag when the lookup settles", () => {
    expect(screen).toMatch(/onSettled:\s*\(\) =>\s*\{\s*inFlight\.current = false;/);
  });

  it("hands the form the held-Enter handler, and the form puts it on the <form>", () => {
    expect(screen).toMatch(/onKeyDown=\{ignoreHeldEnter\}/);
    expect(form).toMatch(/onKeyDown=\{props\.onKeyDown\}/);
  });
});
