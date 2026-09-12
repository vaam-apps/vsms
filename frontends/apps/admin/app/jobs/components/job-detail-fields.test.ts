// Guards the mechanical `JobDetailField` -> `DetailRow variant="divided"`
// conversion, which typechecked while being wrong once already.
//
// The first attempt rewrote `value={cond ? a : b}` as a *bare* JSX child.
// That is valid TSX and compiles, but React renders a bare child as text —
// so the drawer would have shown the literal source `lock.pid != null ? ...`
// instead of the value. `tsc` caught exactly one instance, and only by luck
// (a `string | undefined` mismatch two lines away); it would have missed
// every instance whose ternary branches happened to agree on type.
//
// So this asserts the thing a typechecker structurally cannot: what comes
// out. Same `.test.ts` + `createElement` shape as
// `jobs-table.hidebelow.test.ts`, for the same reason documented there —
// this app's tsconfig sets `"jsx": "preserve"`, so JSX syntax is not
// available under plain vitest.
//
// Verified to actually fail, not just pass by construction: reverting one
// row to a bare (unbraced) child reproduced a real failure before the
// braced form was restored — see this commit's message.

import * as React from "react";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { JobDetailFields } from "./job-detail-fields";
import type { JobListItem } from "./jobs-table";

(globalThis as unknown as { React: typeof React }).React = React;

const JOB = {
  id: "cjob0000000000000000001",
  kind: "expire_stale",
  state: "pending",
  attempts: 2,
  maxAttempts: 5,
  priority: 100,
  runAt: "2026-08-14T12:00:00.000Z",
  lastError: null,
  version: 1,
  // Non-nullable on the wire, and rendered unguarded by `JobDetailFields`
  // (unlike `leaseUntil`/`startedAt`/`finishedAt`, which it `!= null`-checks).
  // The `as unknown as` cast below means TypeScript never noticed they were
  // missing, so `TimestampDisplay` was being handed `undefined` and rendering
  // an invalid date -- silently, until @vaam-apps/ui 0.1.2 made
  // `formatAbsolute` throw on one instead of formatting it. The fixture was
  // always wrong; 0.1.2 is what made it say so.
  createdAt: "2026-08-14T11:59:00.000Z",
  updatedAt: "2026-08-14T12:00:00.000Z",
} as unknown as JobListItem;

const html = renderToStaticMarkup(createElement(JobDetailFields, { job: JOB }));

describe("JobDetailFields", () => {
  // Pins the exact treatment `@vaam-apps/ui`'s `DetailRow` renders. That
  // treatment is the package's to change, not this repo's: 0.1.2 added
  // `min-w-0` to the row and `min-w-0 break-words` to the value cell, an
  // overflow fix for long values. So a diff here on a version bump is
  // expected and is read, not silenced -- what it must never show is the
  // structure changing (a `<div>`/`<dt>`/`<dd>` disappearing), which is the
  // half this test shares with the expression-source check below.
  it("keeps the divided row treatment unchanged", () => {
    expect(html).toContain(
      'class="flex min-w-0 flex-col gap-0.5 border-edge-subtle border-b py-2 last:border-b-0"',
    );
    expect(html).toContain('<dt class="text-caption text-subtle-foreground">');
    expect(html).toContain('<dd class="min-w-0 break-words text-body text-foreground">');
  });

  // The bug this file exists for: no expression source may reach the DOM.
  it("renders values, never the source text of the expression producing them", () => {
    expect(html).toContain("expire_stale");
    expect(html).toContain("2/5");
    for (const leak of ["!= null ?", "job.attempts", "job.kind", "job.priority"]) {
      expect(html).not.toContain(leak);
    }
  });

  it("renders one dt per dd, inside a single list", () => {
    expect(html.match(/<dt/g)?.length).toBe(html.match(/<dd/g)?.length);
    expect(html.match(/<dl/g)?.length).toBe(1);
  });
});
