import { strict as assert } from "node:assert";
import { copyFileSync, mkdtempSync, readdirSync, readFileSync, rmSync } from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { loadWasm } from "../dist/wasm.js";
import { loadResponderModule, newTestIdentity } from "../support/responder.ts";

const wasmDir = fileURLToPath(new URL("../wasm/", import.meta.url));

test("the sealing module loads", () => {
  assert.equal(typeof loadWasm().Sealer.fromSeed, "function");
  assert.equal(typeof loadWasm().Sealer.fromCallback, "function");
  assert.equal(typeof loadWasm().Sealer.fromEsp256Callback, "function");
});

// A bundler that inlines the glue (Next's Turbopack does) rewrites `__dirname`
// to a build-time path, so a module that reads its `.wasm` from beside itself
// fails at run time with ENOENT. vpay's shop did exactly that and never
// redirected a payer to hosted checkout.
test("the glue carries its own bytes: no file read, no __dirname", () => {
  const glue = readFileSync(join(wasmDir, "vsms_sdk_wasm.js"), "utf8");
  assert.equal(glue.includes("__dirname"), false);
  assert.equal(glue.includes("readFileSync"), false);
});

test("the package ships no separate .wasm file, because the glue holds it", () => {
  const files = readdirSync(wasmDir);
  assert.deepEqual(
    files.filter((name) => name.endsWith(".wasm")),
    [],
  );
});

test("the glue loads from a directory that holds no .wasm file", () => {
  const dir = mkdtempSync(join(tmpdir(), "vsms-wasm-"));
  try {
    for (const name of ["vsms_sdk_wasm.js", "package.json"]) {
      copyFileSync(join(wasmDir, name), join(dir, name));
    }
    const alone = createRequire(import.meta.url)(join(dir, "vsms_sdk_wasm.js")) as {
      Sealer: { fromSeed: unknown };
    };
    assert.equal(typeof alone.Sealer.fromSeed, "function");
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

// `seal` is a promise that nothing was sent. An answer is only ever opened after
// the request went out, so no failure of opening it may carry that name.
test("the module calls an answer it received and cannot read `unreadable`, never `seal`", async () => {
  const identity = newTestIdentity();
  const responder = new (loadResponderModule().Responder)(
    identity.serverSeed,
    [identity.clientPublic],
    "vsms",
  );
  const unreadable = (failure: { kind: string }) => failure.kind === "unreadable";

  async function answerTo(tamper: (headers: [string, string][]) => [string, string][]) {
    const sealer = loadWasm().Sealer.fromSeed(
      identity.clientSeed,
      identity.serverPublic,
      identity.nextPublic,
    );
    const sealed = await sealer.sealCall(
      "POST",
      "/$procs/sendMessage",
      [],
      "{}",
      undefined,
      undefined,
    );
    const opened = await responder.openRequest(
      "POST",
      sealed.target,
      undefined,
      sealed.headers,
      sealed.body,
    );
    const answer = await opened.sealResponse(200, "{}");
    return { sealed, headers: tamper(answer.headers), body: answer.body };
  }

  // A payload type that is not the one asked for.
  const other = await answerTo((headers) =>
    headers.map(([name, value]) => [
      name,
      name.toLowerCase() === "cratestack-payload-type" ? "text/plain" : value,
    ]),
  );
  await assert.rejects(other.sealed.openResponse(200, other.headers, other.body), unreadable);

  // A header that is not a header.
  const malformed = await answerTo((headers) => [...headers, ["not a header name", "x"]]);
  await assert.rejects(
    malformed.sealed.openResponse(200, malformed.headers, malformed.body),
    unreadable,
  );

  // The same call opened twice: the second has nothing left to open.
  const twice = await answerTo((headers) => headers);
  await twice.sealed.openResponse(200, twice.headers, twice.body);
  await assert.rejects(twice.sealed.openResponse(200, twice.headers, twice.body), unreadable);
});
