// What the Delivery lookup keeps in the address, held by reading the screen's
// source (the way `pairing-ui-no-dropdowns.test.ts` reads its files): the URL
// carries `by` and `detail` and nothing else. A phone number in a query string
// is kept by the browser's history, by server access logs and by a pasted link,
// so no key may ever be added for it, or for any other value the person types.
//
// A source scan and not a render because the screen is a hook-driven client
// component, and this app's tests have no DOM to mount it in.

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const HERE = fileURLToPath(new URL(".", import.meta.url));

function code(file: string): string {
  return readFileSync(`${HERE}${file}`, "utf8")
    .replace(/\/\*[\s\S]*?\*\//g, "")
    .replace(/^\s*\/\/.*$/gm, "");
}

/** The keys of every `useQueryStates({ ... })` call in `source`. */
function queryStateKeys(source: string): string[][] {
  const calls = [...source.matchAll(/useQueryStates\(\s*\{([\s\S]*?)\}\s*,/g)];
  return calls.map(([, body]) =>
    [...(body ?? "").matchAll(/(\w+)\s*:\s*parseAs/g)].map((match) => match[1] ?? ""),
  );
}

describe("the Delivery lookup's address", () => {
  const screen = code("delivery-lookup-screen.tsx");

  it("holds exactly `by` and `detail`", () => {
    expect(queryStateKeys(screen)).toEqual([["by", "detail"]]);
  });

  it("uses no other way to put state in the address", () => {
    expect(screen).not.toMatch(
      /useSearchParams|router\.(push|replace)|history\.(push|replace)State|useQueryState\(/,
    );
  });

  it("hands the address setter only the kind, or a result's message id", () => {
    // `messageId` below is the id of a row the server returned, never the
    // field the person typed into (that one is `submitted.messageId`).
    const allowed = ["{ by: next, detail: null }", "{ detail: null }", "{ detail: messageId }"];
    const calls = [...screen.matchAll(/setUrlState\(([^)]*)\)/g)];
    expect(calls.length).toBeGreaterThan(0);
    for (const call of calls) expect(allowed, call[0]).toContain((call[1] ?? "").trim());
  });

  it("the scan can see a key: a screen with a third one is read as having it", () => {
    const sabotaged = screen.replace(
      "{ by: parseAsString, detail: parseAsString }",
      "{ by: parseAsString, detail: parseAsString, msisdn: parseAsString }",
    );
    expect(queryStateKeys(sabotaged)).toEqual([["by", "detail", "msisdn"]]);
  });
});
