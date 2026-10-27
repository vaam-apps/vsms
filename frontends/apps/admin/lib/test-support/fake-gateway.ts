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
  /** The seed the gateway seals answers with. */
  serverSeed: string;
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
    serverSeed: seed,
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

// ---- the gateway's word on who is composing ----------------------------------

/** Who a key stands as at the gateway: what `SignerDirectory::standing` makes of it. */
export interface Principal {
  kind: "user" | "app";
  id: string;
  role: string;
  /** A user's role permissions, or an app's scopes. */
  perms: string[];
}

/** A key the gateway has registered, and who it stands as. */
export interface Signer {
  /** Unpadded base64url: 32 bytes for Ed25519, the 65-byte SEC1 point for ESP256. */
  publicKey: string;
  principal: Principal;
}

/** The roles `authorizeComposition`'s `@allow` admits. */
const COMPOSER_ROLES = ["owner", "admin", "operator"];

/** `authorizeComposition` as the gateway decides it
 *  (`backends/crates/sms-api/src/composition.rs`, and its `@allow` in
 *  `schemas/vsms.cstack`): Layer 1 a user of one of three roles, Layer 2 the
 *  `sms:send` permission, then the arguments. The Rust live suite
 *  (`authorize_composition_live_postgres.rs`) holds the real one to the same
 *  table; this is the fake the console's tests stand a gateway up with. */
export function authorizeComposition(principal: Principal, payload: string): Reply {
  const refuse = (status: number, message: string): Reply => ({
    status,
    body: JSON.stringify({ code: status === 403 ? "FORBIDDEN" : "VALIDATION_ERROR", message }),
  });
  if (principal.kind !== "user" || !COMPOSER_ROLES.includes(principal.role)) {
    return refuse(403, "procedure policy denied this operation");
  }
  if (!principal.perms.includes("sms:send")) {
    return refuse(403, 'missing required permission "sms:send"');
  }
  const { args } = JSON.parse(payload) as { args: { operation: string; inputSha256: string } };
  if (args.operation !== "send" && args.operation !== "preview") {
    return refuse(422, "operation must be `send` or `preview`");
  }
  if (!/^[0-9a-f]{64}$/.test(args.inputSha256)) {
    return refuse(422, "inputSha256 must be 64 lowercase hexadecimal digits");
  }
  return {
    status: 200,
    body: JSON.stringify({
      userId: principal.id,
      role: principal.role,
      operation: args.operation,
      inputSha256: args.inputSha256,
    }),
  };
}

/** What a composer route sees of the gateway: the request it was forwarded, and
 *  a sealed answer made by the gateway's key (or the coarse unsigned 401). */
export interface ComposeUpstream {
  (request: {
    method: string;
    target: string;
    headers: [string, string][];
    body: Uint8Array;
  }): Promise<{ status: number; headers: Headers; body: Uint8Array }>;
  /** Every target it was asked for. */
  seen: string[];
}

/** A gateway that knows exactly `signers`, answers `authorizeComposition` as
 *  the real one does, and refuses everything it cannot verify with the coarse
 *  unsigned 401. One verifier per key: the key that opens the request is who
 *  signed it, and each verifier refuses a nonce it has seen. */
export function composeUpstream(gateway: FakeGateway, signers: Signer[]): ComposeUpstream {
  const verifiers = signers.map(({ publicKey, principal }) => ({
    principal,
    responder: new gateway.module.Responder(gateway.serverSeed, [publicKey], "vsms"),
  }));
  const upstream = (async ({ method, target, headers, body }) => {
    upstream.seen.push(target);
    const [path = "", query] = target.split("?");
    for (const { principal, responder } of verifiers) {
      let opened: WasmOpenedRequest;
      try {
        opened = await responder.openRequest(method, path, query, headers, body);
      } catch {
        continue;
      }
      const reply =
        path === "/$procs/authorizeComposition"
          ? authorizeComposition(principal, opened.payload)
          : { status: 404, body: '{"code":"NOT_FOUND"}' };
      const sealed = await opened.sealResponse(reply.status, reply.body);
      return { status: reply.status, headers: new Headers(sealed.headers), body: sealed.body };
    }
    return {
      status: 401,
      headers: new Headers({ "content-type": "text/plain" }),
      body: new TextEncoder().encode(UNSIGNED_401),
    };
  }) as ComposeUpstream;
  upstream.seen = [];
  return upstream;
}

/** The answer-opener the composer is given, over the test module's: failures
 *  take the shape of the SDK's `SdkError` (`@vaam-apps/vsms-node`'s
 *  `answerFailure`), which is what the composer reads. */
export function composeOpener(gateway: FakeGateway) {
  const opener = gateway.module.Opener.forServerKeys(
    gateway.config.serverKeys.current,
    gateway.config.serverKeys.next,
  );
  return {
    async open(
      request: { method: string; target: string; body: Uint8Array; headers?: [string, string][] },
      answer: { status: number; headers: [string, string][]; body: Uint8Array },
    ): Promise<string> {
      const [path = "", query] = request.target.split("?");
      try {
        return await opener.openAnswer(
          request.method,
          path,
          query,
          request.body,
          request.headers ?? [],
          answer.status,
          answer.headers,
          answer.body,
        );
      } catch (failure) {
        const { kind, message, status } = failure as {
          kind: string;
          message: string;
          status?: number;
        };
        throw Object.assign(new Error(message), {
          name: "SdkError",
          kind,
          httpStatus: status ?? answer.status,
        });
      }
    },
  };
}
