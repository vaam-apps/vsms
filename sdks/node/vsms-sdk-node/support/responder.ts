/**
 * The server half of the transport, for tests: `sdks/rust/vsms-sdk-wasm` built
 * with its `responder` feature (`pnpm run build:wasm-test`) into `wasm-test/`,
 * which the npm package does not ship.
 *
 * It opens a sealed request the way the gateway does (finds the operation,
 * rebuilds the binding the client signed, verifies, refuses a replay) and seals
 * an answer under the server's key, using cratestack's own server envelope. That
 * is the point: the SDK's suites run against a verifier that is not a copy of
 * the SDK's sealing code. The real `EnvelopeLayer` is exercised against the same
 * SDK by `live/gateway.live.ts` and by `vsms-sdk-rust`'s `live_gateway` suite.
 */
import { randomBytes } from "node:crypto";
import { createRequire } from "node:module";
import type { HeaderPairs, WasmModule } from "../src/wasm.ts";

export interface OpenedRequest {
  readonly payload: string;
  readonly idempotencyKey: string | undefined;
  readonly route: string;
  readonly pathParams: string[];
  sealResponse(status: number, body: string): Promise<{ headers: HeaderPairs; body: Uint8Array }>;
}

export interface WasmResponder {
  openRequest(
    method: string,
    path: string,
    query: string | undefined,
    headers: HeaderPairs,
    body: Uint8Array,
  ): Promise<OpenedRequest>;
}

export interface ResponderModule extends WasmModule {
  Responder: {
    new (serverSeed: string, clientKeys: string[], audience: string): WasmResponder;
    serverPublicKey(serverSeed: string): string;
  };
}

const require = createRequire(import.meta.url);

let loaded: ResponderModule | undefined;

export function loadResponderModule(): ResponderModule {
  loaded ??= require("../wasm-test/vsms_sdk_wasm.js") as ResponderModule;
  return loaded;
}

/** Keys drawn fresh for one test. Never a canned fixture: nothing here can be a real credential. */
export interface TestIdentity {
  /** The app's Ed25519 seed, unpadded base64url. */
  clientSeed: string;
  /** The registered public half. */
  clientPublic: string;
  /** The server's current seed. */
  serverSeed: string;
  /** What a client pins as `serverKeys.current`. */
  serverPublic: string;
  /** The server's next seed, which a rotation promotes. */
  nextSeed: string;
  /** What a client pins as `serverKeys.next`. */
  nextPublic: string;
}

export function newTestIdentity(): TestIdentity {
  const wasm = loadResponderModule();
  const clientSeed = randomBytes(32).toString("base64url");
  const serverSeed = randomBytes(32).toString("base64url");
  const nextSeed = randomBytes(32).toString("base64url");
  const serverPublic = wasm.Responder.serverPublicKey(serverSeed);
  const nextPublic = wasm.Responder.serverPublicKey(nextSeed);
  const clientPublic = wasm.Sealer.fromSeed(clientSeed, serverPublic, nextPublic).publicKey;
  return { clientSeed, clientPublic, serverSeed, serverPublic, nextSeed, nextPublic };
}
