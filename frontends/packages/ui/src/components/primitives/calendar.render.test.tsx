import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { Calendar } from "./calendar";

/**
 * Renders `Calendar` and asserts the styling actually lands on the
 * elements react-day-picker emits.
 *
 * # Why the sibling key-name test was not enough
 *
 * `calendar.test.ts` checks that every `classNames` key is a real
 * react-day-picker element. That is necessary and insufficient, and this
 * file exists because the gap between the two shipped a broken calendar:
 * every key was valid, every class string was valid CSS, and **none of
 * the selection styling matched anything**, because it was written
 * against a DOM shape react-day-picker does not produce —
 * `aria-selected` on the day *button* (it is on the `<td>`) and
 * `data-range-middle` attributes (they do not exist; range state arrives
 * as classes on the cell). Neither `tsc` nor the key test can see that.
 * Rendering can.
 *
 * Deliberately `renderToStaticMarkup` rather than a DOM testing library:
 * the question is which classes land on which elements, which the markup
 * answers exactly, and it needs no `jsdom` and no new dependency —
 * `react-dom` is already here as a peer.
 *
 * What this still cannot see is whether a class *resolves to a visible
 * colour*: the same bug hunt found `bg-state-neutral-bg` on the range
 * interior, a real token that resolves to `transparent`, so the band was
 * invisible. That needs a browser, and the note on `range_middle` in
 * `calendar.tsx` records it.
 */

/** `2026-09-03` … `2026-09-14`, local midnight — the same convention
 * `date-picker.tsx` uses, and never `new Date("2026-09-03")`, which the
 * spec parses as UTC and which lands on the previous day west of UTC. */
const from = new Date(2026, 8, 3);
const to = new Date(2026, 8, 14);

function rangeMarkup(): string {
  return renderToStaticMarkup(
    <Calendar mode="range" selected={{ from, to }} defaultMonth={from} numberOfMonths={1} />,
  );
}

/**
 * The `<td>` whose day button is labelled for `day` in September 2026,
 * with HTML entities decoded.
 *
 * The decode is not cosmetic: Tailwind's arbitrary variants are full of
 * `&` and `>`, which `renderToStaticMarkup` escapes into `&amp;` and
 * `&gt;`. Without it, asserting on the class a developer actually wrote
 * (`[&>button]:bg-transparent!`) fails against markup that genuinely
 * contains it — which cost this file its own first run.
 */
function cellFor(markup: string, day: number): string {
  const cells = markup
    .split("<td")
    .slice(1)
    .map((c) => `<td${c.split("</td>")[0]}`);
  const match = cells.find((c) => new RegExp(`>${day}</button>`).test(c));
  if (match === undefined) throw new Error(`no cell rendered for September ${day}`);
  return match.replaceAll("&amp;", "&").replaceAll("&gt;", ">").replaceAll("&lt;", "<");
}

/**
 * Just the `<td>`'s own `class` attribute, excluding the button nested
 * inside it.
 *
 * Needed because `cellFor` returns the whole cell including its child,
 * so a substring assertion against it can pass for the wrong reason:
 * observed directly while proving these guards fail — with the original
 * `aria-selected:bg-primary` bug reinstated on the BUTTON, "fills both
 * ends of the range" still passed, because the button's class contained
 * the substring `bg-primary`.
 */
function cellClassOf(markup: string, day: number): string {
  const cell = cellFor(markup, day);
  return /^<td class="([^"]*)"/.exec(cell)?.[1] ?? "";
}

describe("Calendar renders its selection styling onto real elements", () => {
  const markup = rangeMarkup();

  it("renders a grid at all", () => {
    expect(markup).toContain("<table");
    // Sanity: a September with the range in it, not an empty shell.
    expect(markup).toContain(">14</button>");
  });

  it("puts the selection state on the cell, not the button", () => {
    // The mistake this guards: styling the button with `aria-selected:`.
    const start = cellFor(markup, 3);
    expect(start).toContain("data-selected");
    const button = start.slice(start.indexOf("<button"));
    expect(button).not.toContain("aria-selected");
  });

  it("fills both ends of the range", () => {
    for (const day of [3, 14]) {
      expect(cellClassOf(markup, day), `September ${day}`).toContain("[&>button]:bg-primary");
    }
  });

  it("rounds the outer corners of the range, and only those", () => {
    expect(cellClassOf(markup, 3)).toContain("rounded-l-sm");
    expect(cellClassOf(markup, 14)).toContain("rounded-r-sm");
    expect(cellClassOf(markup, 8)).not.toContain("rounded-l-sm");
    expect(cellClassOf(markup, 8)).not.toContain("rounded-r-sm");
  });

  it("tints the interior cell and clears the button inside it", () => {
    const middle = cellClassOf(markup, 8);
    // The tint is on the CELL so consecutive days meet with no gap...
    expect(middle).toContain("bg-primary/15");
    // ...and the button's own fill is knocked out, importantly, because
    // an interior day also carries `selected`.
    expect(middle).toContain("[&>button]:bg-transparent!");
  });

  it("leaves days outside the range unstyled", () => {
    for (const day of [2, 15]) {
      expect(cellFor(markup, day), `September ${day}`).not.toContain("data-selected");
      expect(cellClassOf(markup, day), `September ${day}`).not.toContain("bg-primary");
    }
  });

  it("renders a single selected day in single mode", () => {
    const single = renderToStaticMarkup(
      <Calendar mode="single" selected={from} defaultMonth={from} />,
    );
    expect(cellFor(single, 3)).toContain("data-selected");
    expect(cellClassOf(single, 3)).toContain("[&>button]:bg-primary");
    expect(cellFor(single, 4)).not.toContain("data-selected");
  });
});
