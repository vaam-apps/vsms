import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { Animation, DayFlag, SelectionState, UI } from "react-day-picker";
import { describe, expect, it } from "vitest";

/**
 * `react-day-picker` accepts a `classNames` record keyed by its own
 * element names and **silently ignores anything it does not recognise** —
 * no error, no warning, no type error either, because the prop's type is
 * a `Partial<Record<…>>` over a wide union and a typo lands in the part
 * of the union that happens to exist.
 *
 * `Calendar` styles the whole calendar through that record, so a renamed
 * key in a future `react-day-picker` major would not break the build: it
 * would produce an unstyled calendar, visible only to whoever next opens
 * a date picker. That is the exact shape of failure this package has
 * already recorded twice in its own stylesheet (`--color-surface-1`,
 * `--state-warning-*`), so it gets the same treatment — a test rather
 * than a promise to remember.
 *
 * Deliberately parses the source rather than importing the component and
 * inspecting props: reading it requires a renderer, and the thing under
 * test is the literal key set an author typed, which the source has and a
 * rendered tree does not.
 */
const SOURCE = readFileSync(fileURLToPath(new URL("./calendar.tsx", import.meta.url)), "utf8");

/** Everything `classNames` legitimately accepts. */
const VALID = new Set<string>([
  ...Object.values(UI),
  ...Object.values(DayFlag),
  ...Object.values(SelectionState),
  ...Object.values(Animation),
]);

/** The keys of the object literal passed as `classNames={{ … }}`. */
function classNameKeys(): string[] {
  const start = SOURCE.indexOf("classNames={{");
  expect(start, "calendar.tsx must pass a classNames object literal").toBeGreaterThan(-1);
  const body = SOURCE.slice(start, SOURCE.indexOf("...classNames", start));
  // Keys at the literal's own indent level — deeper nested selectors like
  // `[&>button]` live inside class strings, not as keys.
  //
  // `[A-Za-z_0-9]+`, not `[a-z_0-9]+`: react-day-picker's own keys are all
  // snake_case, so a lowercase-only pattern silently skips exactly the
  // most likely typo — a camelCase `dayButton` for `day_button`. Proven,
  // not assumed: with the narrower pattern that sabotage was scanned past
  // entirely and caught only by the required-elements assertion below.
  return [...body.matchAll(/^ {8}([A-Za-z_0-9]+):/gm)].map((m) => m[1] as string);
}

describe("Calendar's classNames keys are real react-day-picker elements", () => {
  const keys = classNameKeys();

  it("finds keys to check (guards a regex that silently matches nothing)", () => {
    expect(keys.length).toBeGreaterThan(10);
  });

  it.each(keys)("%s is a known element", (key) => {
    expect(VALID).toContain(key);
  });

  it("styles the elements that carry the calendar's whole visual identity", () => {
    // A subset chosen because losing any one of them makes the calendar
    // look broken rather than merely different.
    for (const required of [UI.Day, UI.DayButton, UI.MonthGrid, UI.Weekday, UI.CaptionLabel]) {
      expect(keys).toContain(required);
    }
  });
});
