"use client";

// The browser's signing transport: `sdks/rust/vsms-sdk-wasm` compiled to
// WebAssembly with `--target web` (`bash sdks/node/vsms-sdk-node/scripts/
// build-wasm.sh --web`, written to `public/sealer/` and served by this console's
// own origin), handed the browser key's public half and a callback that signs
// with the private half through WebCrypto. This is the only module that knows
// the wasm module's shape; everything above it sees `SealingRuntime`.
//
// The key never enters the module: `Sealer.fromEsp256Callback` is given the
// bytes to sign and returns the signature, and the module checks that signature
// verifies under the public key before it uses it. WebCrypto signs P-256 as raw
// `r ‖ s`; the module accepts that and DER, and normalises to the low-`s` form
// the gateway requires of a COSE signature.
//
// The module is imported by URL at run time, not bundled: it is generated code
// (gitignored) the bundler must not try to resolve, and loading it from
// `/sealer/` is what lets a content-security policy say `script-src 'self'`.

import { NoDeviceKeyError, type SealingRuntime, type WasmSealer } from "@vsms/gateway";
import type { ConsoleConfig } from "./console-config";
import { type DeviceKey, loadDeviceKey } from "./device-key";
import { base64Url } from "./pairing";

/** The wasm module's exports (sdks/rust/vsms-sdk-wasm/src/js.rs). */
export interface SealerModule {
  Sealer: {
    fromEsp256Callback(
      publicKey: string,
      sign: (tbs: Uint8Array) => Promise<Uint8Array>,
      serverCurrent: string,
      serverNext: string,
    ): WasmSealer & { readonly publicKey: string; readonly algorithm: string; free(): void };
  };
  routeOf(method: string, path: string): { route: string; params: string[] };
}

export const MODULE_URL = "/sealer/vsms_sdk_wasm.js";
export const WASM_URL = "/sealer/vsms_sdk_wasm_bg.wasm";

/** The console was started without the gateway's keys. */
export class ConsoleNotConfiguredError extends Error {
  constructor() {
    super(
      "the console does not know the gateway it signs for: SMS_ENVELOPE_SERVER_KEY_CURRENT and " +
        "SMS_ENVELOPE_SERVER_KEY_NEXT are not set",
    );
    this.name = "ConsoleNotConfiguredError";
  }
}

/** Builds the runtime for `key`, given the loaded module. Pure of I/O, so a
 *  test hands it the module it loaded itself. */
export function createRuntime(
  module: SealerModule,
  key: Pick<DeviceKey, "publicKey" | "sign">,
  config: ConsoleConfig,
): SealingRuntime {
  const sealer = module.Sealer.fromEsp256Callback(
    base64Url(key.publicKey),
    (tbs) => key.sign(tbs),
    config.serverKeys.current,
    config.serverKeys.next,
  );
  return { sealer, routeOf: (method, path) => module.routeOf(method, path) };
}

let config: ConsoleConfig | undefined;
let runtime: Promise<SealingRuntime> | undefined;
let moduleLoad: Promise<SealerModule> | undefined;

/** Called once by the providers, before anything signs. */
export function configureSealer(next: ConsoleConfig | undefined): void {
  config = next;
  runtime = undefined;
}

function loadModule(): Promise<SealerModule> {
  moduleLoad ??= (async () => {
    // A variable, with the bundlers' ignore comments: the file is generated
    // output the build must leave exactly where it is.
    const url: string = MODULE_URL;
    const loaded = (await import(
      /* webpackIgnore: true */ /* turbopackIgnore: true */ url
    )) as SealerModule & { default(input: { module_or_path: string }): Promise<unknown> };
    await loaded.default({ module_or_path: WASM_URL });
    return loaded;
  })().catch((error: unknown) => {
    moduleLoad = undefined;
    throw error;
  });
  return moduleLoad;
}

async function build(): Promise<SealingRuntime> {
  if (!config) throw new ConsoleNotConfiguredError();
  const key = await loadDeviceKey();
  if (!key) throw new NoDeviceKeyError();
  return createRuntime(await loadModule(), key, config);
}

/** The signing runtime for this browser's key. A browser with no key rejects
 *  with {@link NoDeviceKeyError}; only the pairing page makes a key. Memoised,
 *  and re-tried after a failure. */
export function getRuntime(): Promise<SealingRuntime> {
  runtime ??= build().catch((error: unknown) => {
    runtime = undefined;
    throw error;
  });
  return runtime;
}

/** Drops the runtime, so the next {@link getRuntime} reads the key store afresh:
 *  after pairing, unpairing or replacing the key. */
export function forgetSealer(): void {
  const dropped = runtime;
  runtime = undefined;
  dropped?.then(({ sealer }) => (sealer as { free?: () => void }).free?.()).catch(() => {});
}
