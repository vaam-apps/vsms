#!/usr/bin/env node
// The drift gate for T3 (M4 #47's underlying concern) over the freshly
// generated `frontends/packages/sms-client` — see frontends/packages/sms-client/GENERATING.md.
//
// `frontends/packages/sms-client` is entirely generated at build/CI time and is not
// committed (the owner's standing rule: generated code never goes into
// version control — see the repo root .gitignore's comment on that
// package). There used to be a second gate here, reimplementing
// `cratestack generate-typescript --check`'s job of proving "the committed
// client matches the schema" — with nothing committed, that gate would
// have nothing to diff against and would assert nothing, so it was
// removed rather than kept as decoration.
//
// This script is the gate that still means something: the `cratestack`
// CLI used to generate this client (must be the *published* release, per
// CI's own install step, but could still be a newer minor than what the
// server binary was built from) can diverge from the library family
// actually compiled into `sms-gateway`, which is pinned `=0.6.7` (see
// AGENTS.md's "one environment note worth recording" — the same skew
// already observed to make a *newer* CLI's `migrate diff` emit FK DDL the
// *pinned* `migrate` library never produces, on the unmodified schema). A
// CLI/library route-shape drift would otherwise 404 every request at
// runtime, exactly the trap `--base-path ''` already warns about for the
// base path specifically.
//
// So this builds the real server binary from the pinned library, asks it
// for its own route table (`sms-gateway routes`), and asserts every HTTP
// call the generated client can make (extracted straight from
// `frontends/packages/sms-client/src/client.ts`, not re-derived from the schema)
// names a route that server table actually serves. It is a plain set
// comparison against ~102 lines — cheap, and it is the only gate here
// that would catch this specific skew.
//
// This is intentionally one-directional: it does not require every server
// route to have a client caller (some models may legitimately be
// console-only until a later task wires them up). It only asserts the
// client never calls a route the server doesn't have.

import { execFileSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..");
const clientPath = join(repoRoot, "frontends/packages/sms-client/src/client.ts");

function serverRoutes() {
  const binary = join(repoRoot, "target/debug/sms-gateway");
  if (!existsSync(binary)) {
    console.log("building sms-gateway (target/debug/sms-gateway not found)...");
    execFileSync("cargo", ["build", "-p", "sms-gateway"], {
      cwd: repoRoot,
      stdio: "inherit",
    });
  }

  const output = execFileSync(binary, ["routes"], {
    cwd: repoRoot,
    encoding: "utf8",
  });

  const routes = new Set();
  for (const line of output.split("\n")) {
    const match = line.match(/^\s*(GET|POST|PATCH|DELETE|PUT)\s+(\S+)\s*$/);
    if (!match) continue;
    const [, method, path] = match;
    routes.add(`${method} ${path}`);
  }

  if (routes.size === 0) {
    throw new Error(
      "sms-gateway routes produced zero parseable route lines — output format changed? " +
        "This gate parses lines shaped 'METHOD    /path'.",
    );
  }

  return routes;
}

function clientCalls() {
  if (!existsSync(clientPath)) {
    throw new Error(`${clientPath} not found — run 'just client-gen' before 'just client-check'.`);
  }

  const source = readFileSync(clientPath, "utf8");
  const methodMap = { get: "GET", post: "POST", patch: "PATCH", delete: "DELETE" };

  // Matches `this.runtime.<method><...generic...>(<whitespace/newlines><literal>`.
  // The generic argument (e.g. `Page<App>`) may itself contain `<...>`; the
  // lazy `[\s\S]*?>\(` still resolves correctly because the literal
  // substring ">(" occurs exactly once, immediately before the call's
  // first argument, regardless of nesting depth inside the generic.
  const callPattern = /this\.runtime\.(get|post|patch|delete)<[\s\S]*?>\(\s*(`[^`]*`|"[^"]*")/g;

  const calls = [];
  for (const match of source.matchAll(callPattern)) {
    const [, runtimeMethod, literal] = match;
    const method = methodMap[runtimeMethod];
    const lineNumber = source.slice(0, match.index).split("\n").length;

    // Strip the surrounding quote/backtick, then collapse any `${...}`
    // interpolation (always an encoded path param, e.g.
    // `${encodeURIComponent(String(id))}`) down to the server's own
    // `{id}` placeholder spelling.
    const rawPath = literal.slice(1, -1);
    const path = rawPath.replace(/\$\{[^}]*\}/g, "{id}");

    calls.push({ method, path, lineNumber });
  }

  if (calls.length === 0) {
    throw new Error(
      `Found zero 'this.runtime.<method>(...)' calls in ${clientPath} — ` +
        "extraction pattern is stale, or the client was generated empty.",
    );
  }

  return calls;
}

function main() {
  const server = serverRoutes();
  const calls = clientCalls();

  const missing = calls.filter(({ method, path }) => !server.has(`${method} ${path}`));

  if (missing.length > 0) {
    console.error(
      `assert-client-routes-match-server: ${missing.length} client call(s) name a route ` +
        "the pinned server (sms-gateway, cratestack =0.6.7) does not serve:\n",
    );
    for (const { method, path, lineNumber } of missing) {
      console.error(`  ${method.padEnd(6)} ${path}    (client.ts:${lineNumber})`);
    }
    console.error(
      "\nThis is the CLI/library-skew gate (see this file's header comment). " +
        "Either the client was generated with a newer cratestack CLI than the pinned " +
        "server library, or a route path in client.ts was hand-edited. Regenerate with " +
        "'just client-gen' and re-run 'cargo build -p sms-gateway' before re-checking.",
    );
    process.exit(1);
  }

  console.log(
    `assert-client-routes-match-server: OK — all ${calls.length} client call(s) match ` +
      `routes served by the pinned sms-gateway binary (${server.size} routes total).`,
  );
}

main();
