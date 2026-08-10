#!/usr/bin/env node
// #179: the "mermaid diagrams parse" CI job used to install
// @mermaid-js/mermaid-cli, whose puppeteer postinstall downloads a Chrome
// headless-shell build from an upstream CDN at job runtime. It failed on
// #178 — a PR that touched no diagrams at all — purely because that
// download failed. The job's stated job is "diagrams parse", not "diagrams
// render to SVG"; rendering is what needed a browser, not parsing.
//
// mermaid.parse() runs each diagram's own grammar (jison, for the
// flowchart/sequenceDiagram/stateDiagram-v2 types this doc actually uses)
// without ever touching layout, Canvas measurement, or a real display —
// the only browser-shaped thing it needs is a `document` to construct a
// handful of DOM/SVG elements against. jsdom stands in for that, so the
// whole check runs in plain Node: no Chrome, no CDN, no puppeteer, and
// `mermaid`/`jsdom` are pinned to exact versions in this directory's own
// package.json + package-lock.json, so the only thing left that can make
// this job fail is a diagram that doesn't parse.
import { readFileSync } from "node:fs";
import { JSDOM } from "jsdom";

const docPath = process.argv[2];
if (!docPath) {
  console.error("usage: node parse.mjs <path-to-markdown-file-with-mermaid-blocks>");
  process.exit(2);
}

const dom = new JSDOM("<!DOCTYPE html><html><body></body></html>", { pretendToBeVisual: true });
globalThis.window = dom.window;
globalThis.document = dom.window.document;
globalThis.SVGElement = dom.window.SVGElement;
Object.defineProperty(globalThis, "navigator", { value: dom.window.navigator, configurable: true });

const mermaid = (await import("mermaid")).default;
mermaid.initialize({ startOnLoad: false });

const src = readFileSync(docPath, "utf8");
const blocks = [...src.matchAll(/```mermaid\n([\s\S]*?)```/g)].map((m) => m[1]);

if (blocks.length === 0) {
  console.error(`no mermaid blocks found in ${docPath}`);
  process.exit(1);
}

let failures = 0;
for (const [i, block] of blocks.entries()) {
  try {
    const result = await mermaid.parse(block, { suppressErrors: false });
    console.log(`diagram ${i}: OK (${result.diagramType})`);
  } catch (err) {
    failures += 1;
    console.error(`diagram ${i}: FAILED to parse\n${err.message}\n--- source ---\n${block}`);
  }
}

console.log(`${blocks.length} diagram(s) checked, ${failures} failed`);
process.exit(failures > 0 ? 1 : 0);
