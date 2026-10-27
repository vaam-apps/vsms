// A gateway for tests: a real sealed verifier (cratestack's server envelope, in
// the sealing module's `responder` build) behind a `fetch`-shaped function, so
// the console's code is exercised against bytes a gateway would accept or
// refuse, and answers it would sign. The console's own route handlers sit
// between the browser and the real gateway; here the "pass-through" is this
// function, which is allowed to see exactly what the real one is.

import { randomBytes } from "node:crypto";
import type { SealingRuntime } from "@vsms/gateway";
import { type DeviceKey, loadOrCreateDeviceKey, memoryKeyStore } from "../device-key";
import { base64Url } from "../pairing";
import { createRuntime } from "../sealer";
import {
  loadTestModule,
  type TestModule,
  type WasmOpenedRequest,
  type WasmResponder,
} from "./wasm";

export interface RecordedRequest {
  /** The URL the browser posted to. */
  url: string;
  /** The real method, from `x-vsms-method`. */
  method: string;
  headers: Record<string, string>;
  credentials: RequestCredentials | undefined;
  body: Uint8Array;
}

export interface Reply {
  status: number;
  /** The JSON text the gateway seals. */
  body: string;
}

export type Route = (request: {
  method: string;
  path: string;
  query: string | undefined;
  payload: string;
  ifMatch: string | undefined;
  opened: WasmOpenedRequest;
}) => Reply | Promise<Reply>;

export interface FakeGateway {
  module: TestModule;
  /** The gateway's verifier, for tests that hand it bytes directly. */
  responder: WasmResponder;
  key: DeviceKey;
  runtime: SealingRuntime;
  config: { serverKeys: { current: string; next: string }; messageStreamPollMs: number };
  requests: RecordedRequest[];
  /** What the browser posts to; installs as the sealing fetch's `send`. */
  send: typeof fetch;
  /** Answers the next request(s) with this instead of verifying anything. */
  override: ((request: RecordedRequest) => Response | undefined) | undefined;
  route: Route;
}

const UNSIGNED_401 = "request could not be authenticated";

/** Makes a key, a gateway that knows it (unless `registered` is false), and a
 *  runtime that signs with it. */
export async function startFakeGateway(
  options: { registered?: boolean; route?: Route; key?: DeviceKey } = {},
): Promise<FakeGateway> {
  const module = await loadTestModule();
  const key = options.key ?? (await loadOrCreateDeviceKey(memoryKeyStore()));
  const seed = randomBytes(32).toString("base64url");
  const nextSeed = randomBytes(32).toString("base64url");
  const config = {
    serverKeys: {
      current: module.Responder.serverPublicKey(seed),
      next: module.Responder.serverPublicKey(nextSeed),
    },
    messageStreamPollMs: 500,
  };
  const known = options.registered === false ? [] : [base64Url(key.publicKey)];
  const responder = new module.Responder(seed, known, "vsms");

  const gateway: FakeGateway = {
    module,
    responder,
    key,
    config,
    runtime: createRuntime(module, key, config),
    requests: [],
    override: undefined,
    route: options.route ?? (() => ({ status: 200, body: "{}" })),
    send: undefined as never,
  };

  gateway.send = (async (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
    const url = new URL(
      typeof input === "string" ? input : input.toString(),
      "http://console.test",
    );
    const headers = new Headers(init?.headers);
    const recorded: RecordedRequest = {
      url: `${url.pathname}${url.search}`,
      method: headers.get("x-vsms-method") ?? "",
      headers: Object.fromEntries(headers.entries()),
      credentials: init?.credentials,
      body: init?.body instanceof Uint8Array ? init.body : new Uint8Array(),
    };
    gateway.requests.push(recorded);

    const overridden = gateway.override?.(recorded);
    if (overridden !== undefined) return overridden;

    const path = url.pathname.replace(/^\/api\/gateway/, "");
    const query = url.search === "" ? undefined : url.search.slice(1);
    let opened: WasmOpenedRequest;
    try {
      opened = await responder.openRequest(
        recorded.method,
        path,
        query,
        [...headers.entries()],
        recorded.body,
      );
    } catch {
      // The gateway's coarse refusal: unsigned, and it says nothing of why.
      return new Response(UNSIGNED_401, { status: 401, headers: { "content-type": "text/plain" } });
    }
    const reply = await gateway.route({
      method: recorded.method,
      path,
      query,
      payload: opened.payload,
      ifMatch: headers.get("if-match") ?? undefined,
      opened,
    });
    const sealed = await opened.sealResponse(reply.status, reply.body);
    // What the console's pass-through does with a status that cannot carry a
    // body: answer 200 and name the real one.
    const bodiless = reply.status === 204 || reply.status === 205 || reply.status === 304;
    return new Response(sealed.body as BodyInit, {
      status: bodiless ? 200 : reply.status,
      headers: {
        ...Object.fromEntries(sealed.headers),
        ...(bodiless ? { "x-vsms-status": String(reply.status) } : {}),
      },
    });
  }) as typeof fetch;

  return gateway;
}
