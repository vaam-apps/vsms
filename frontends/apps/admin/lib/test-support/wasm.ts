// The sealing module as the tests load it: `sdks/rust/vsms-sdk-wasm` built with
// `--target web` and its `responder` feature (`pnpm run build:sealer-test`, run
// by `pnpm test`), instantiated synchronously from its bytes. Same crate and
// same wasm-bindgen as the `public/sealer` module the browser loads; the one
// difference is the gateway's server half compiled in, which is what lets a test
// talk to a verifier that shares no code with the browser's sealing path
// (cratestack's own `CoseEnvelope::server`).

import { existsSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import type { SealerModule } from "../sealer";

export interface WasmOpenedRequest {
  readonly payload: string;
  readonly idempotencyKey: string | undefined;
  readonly route: string;
  readonly pathParams: string[];
  sealResponse(
    status: number,
    body: string,
  ): Promise<{ headers: [string, string][]; body: Uint8Array }>;
}

export interface WasmResponder {
  openRequest(
    method: string,
    path: string,
    query: string | undefined,
    headers: [string, string][],
    body: Uint8Array,
  ): Promise<WasmOpenedRequest>;
}

export interface TestModule extends SealerModule {
  Responder: {
    new (serverSeed: string, clientKeys: string[], audience: string): WasmResponder;
    serverPublicKey(seed: string): string;
  };
  initSync(input: { module: Uint8Array }): unknown;
}

const DIRECTORY = fileURLToPath(new URL("../../.sealer-test/", import.meta.url));

let loaded: Promise<TestModule> | undefined;

/** The module, instantiated once per test file. A missing build is a failure
 *  with the command that makes it, not a skipped test: a gate that skips when its
 *  subject was not built is not a gate. */
export function loadTestModule(): Promise<TestModule> {
  loaded ??= (async () => {
    const wasm = `${DIRECTORY}vsms_sdk_wasm_bg.wasm`;
    if (!existsSync(wasm)) {
      throw new Error(
        "the sealing module is not built: run `pnpm --filter admin run build:sealer-test` " +
          "(needs Rust with the wasm32 target and wasm-bindgen at the lockfile's version: `just wasm-tools`)",
      );
    }
    const module = (await import(/* @vite-ignore */ `${DIRECTORY}vsms_sdk_wasm.js`)) as TestModule;
    module.initSync({ module: readFileSync(wasm) });
    return module;
  })();
  return loaded;
}
