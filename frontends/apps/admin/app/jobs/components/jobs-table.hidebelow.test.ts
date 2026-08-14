// Renders the real JobsTable to static HTML (no JSX needed —
// React.createElement directly, so this file can stay a plain `.test.ts`
// under this project's vitest `include` glob) and asserts the exact
// breakpoint class landed on each column's <th>/<td> pair. This is the
// thing neither `tsc` nor a passing build can see: a wrong `hideBelow`
// value still typechecks fine, it just silently changes which columns show
// on a phone — this table used to encode the same decision as four hoisted
// `COL_*` consts (`sm`/`md`/`lg`/`lg` for Attempts/Run at/Last error/Id)
// before the `hideBelow` prop replaced them; this test pins that mapping so
// a future edit can't quietly swap two breakpoints and still pass CI.
//
// Verified to actually fail, not just pass by construction: temporarily
// changing Attempts' `hideBelow` from "sm" to "lg" in jobs-table.tsx
// reproduced `expected '...hidden lg:table-cell' to contain 'sm:table-cell'`
// before being reverted.

import * as React from "react";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { type JobListItem, JobsTable } from "./jobs-table";

// This app's tsconfig sets `"jsx": "preserve"` (Next/SWC does the real
// transform at build time); under plain vitest/esbuild that makes the
// classic `React.createElement` transform apply instead of the automatic
// runtime, and jobs-table.tsx (correctly, for its real Next.js build) never
// imports `React` itself. Satisfy that one free reference for this
// throwaway test only.
(globalThis as unknown as { React: typeof React }).React = React;

const item: JobListItem = {
  id: "job1",
  kind: "expire_stale",
  state: "pending",
  attempts: 0,
  maxAttempts: 5,
  priority: 0,
  dedupeKey: null,
  lastError: null,
  runAt: new Date().toISOString(),
  leaseOwner: null,
  leaseUntil: null,
  startedAt: null,
  finishedAt: null,
  version: 1,
  createdAt: new Date().toISOString(),
  updatedAt: new Date().toISOString(),
} as unknown as JobListItem;

describe("JobsTable hideBelow breakpoints (real render, not just types)", () => {
  it("puts each column's head+cell on the exact breakpoint the original four consts encoded", () => {
    const html = renderToStaticMarkup(
      createElement(JobsTable, {
        items: [item],
        isLoading: false,
        hasFilters: false,
        onClearFilters: () => {},
        onRowClick: () => {},
        onRequeueClick: () => {},
        requeuePending: false,
      }),
    );

    // Head cells, in column order: State, Kind, Attempts, Last error, Run at, Id, Updated, Action
    const heads = [...html.matchAll(/<th class="([^"]*)"[^>]*>(.*?)<\/th>/g)];
    const headByLabel = (label: string) => heads.find((m) => m[2] === label)?.[1] ?? "MISSING";

    expect(headByLabel("Attempts")).toContain("hidden");
    expect(headByLabel("Attempts")).toContain("sm:table-cell");
    expect(headByLabel("Attempts")).not.toContain("md:table-cell");
    expect(headByLabel("Attempts")).not.toContain("lg:table-cell");

    expect(headByLabel("Run at")).toContain("hidden");
    expect(headByLabel("Run at")).toContain("md:table-cell");
    expect(headByLabel("Run at")).not.toContain("sm:table-cell");
    expect(headByLabel("Run at")).not.toContain("lg:table-cell");

    expect(headByLabel("Last error")).toContain("hidden");
    expect(headByLabel("Last error")).toContain("lg:table-cell");
    expect(headByLabel("Last error")).not.toContain("sm:table-cell");
    expect(headByLabel("Last error")).not.toContain("md:table-cell");

    expect(headByLabel("Id")).toContain("hidden");
    expect(headByLabel("Id")).toContain("lg:table-cell");

    // State/Kind/Updated/Action never hide.
    expect(headByLabel("State")).not.toContain("hidden");
    expect(headByLabel("Kind")).not.toContain("hidden");
    expect(headByLabel("Updated")).not.toContain("hidden");
    expect(headByLabel("Action")).not.toContain("hidden");

    // Body cells: same breakpoint per column, read positionally since cell
    // text isn't a stable label the way head text is.
    const cells = [...html.matchAll(/<td class="([^"]*)"/g)].map((m) => m[1]);
    // order: State, Kind, Attempts, Last error, Run at, Id, Updated, Action
    expect(cells[2]).toContain("sm:table-cell");
    expect(cells[3]).toContain("lg:table-cell");
    expect(cells[4]).toContain("md:table-cell");
    expect(cells[5]).toContain("lg:table-cell");
    expect(cells[0]).not.toContain("hidden");
    expect(cells[1]).not.toContain("hidden");
    expect(cells[6]).not.toContain("hidden");
    expect(cells[7]).not.toContain("hidden");
  });
});
