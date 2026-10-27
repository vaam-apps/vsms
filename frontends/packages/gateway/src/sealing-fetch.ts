// A `fetch` that seals. Every module of this package writes the request it
// wants as an ordinary one (`GET /apps?limit=500`, `PATCH /providers/p1` with an
// `If-Match`) and calls `gatewayFetch`; in the console that is this function.
// It hands the route, the body and the two headers a signature binds to the
// sealer (`sdks/rust/vsms-sdk-wasm`, compiled to WebAssembly, signing with the
// browser's non-extractable key), sends the sealed bytes to the console's own
// pass-through, and answers with a `Response` made of what the sealer opened.
// The module above cannot tell the difference, which is the point: nothing in
// the pages knows requests are signed.
//
// What leaves the browser is a COSE message and the headers the seal depends
// on. Never an `Authorization` header, never a cookie (`credentials: "omit"`):
// the sealed request is the credential, and a request that is not one is
// refused by the gateway before it runs.
//
// **Why the pass-through is a `POST` whatever the method.** A sealed `GET`
// carries the COSE message as its body, which `fetch` refuses. The browser
// therefore posts the bytes to `<endpoint>/<path>` and names the real method in
// `x-vsms-method`; the pass-through (`./bff/pass-through.ts`) replays them
// upstream under that method. The method is signed (it is part of the binding),
// so a pass-through that lied about it would produce a request the gateway
// refuses.
//
// **What comes back is believed only if it opens.** `openResponse` verifies the
// answer under one of the two pinned server keys and against this very request.
// An answer that does not (the gateway's coarse unsigned 401, a proxy's error
// page, a response from a host that is not the gateway) is never read: it
// becomes a status of this console's own, in the same JSON error shape, so the
// module above handles it like any other refusal.
//
//   unsigned 401              401 unauthenticated   this browser is not paired (yet)
//   unsigned, anything else   503 unavailable       nothing was read or changed
//   unverified                502 unverified        an answer that failed its seal
//   seal / unbound / invalid  400 invalid_argument  never sent
//   no key in this browser    401 unauthenticated   never sent

import { GATEWAY_ORIGIN, type GatewayFetch } from "./upstream";

/** What the wasm module's `SealedCall` is (sdks/rust/vsms-sdk-wasm/src/js.rs). */
export interface WasmSealedCall {
  readonly body: Uint8Array;
  /** `[name, value]` pairs, to be sent as given. */
  readonly headers: [string, string][];
  /** The path and query to send to, built from the parameters that were bound. */
  readonly target: string;
  openResponse(status: number, headers: [string, string][], body: Uint8Array): Promise<string>;
}

/** The part of the wasm module's `Sealer` this fetch uses. */
export interface WasmSealer {
  sealCall(
    method: string,
    route: string,
    params: string[],
    body: string | undefined,
    query: string | undefined,
    idempotencyKey: string | undefined,
    ifMatch: string | undefined,
  ): Promise<WasmSealedCall>;
}

/** Everything sealing needs, loaded once the browser has a key. */
export interface SealingRuntime {
  sealer: WasmSealer;
  /** The wasm module's `routeOf`: the template and decoded parameters of a
   *  concrete path, or a rejection (`kind: "unbound"`) for one the schema does
   *  not serve. */
  routeOf(method: string, path: string): { route: string; params: string[] };
}

/** What the wasm module rejects with. */
export interface SealerFailure {
  kind: "unbound" | "seal" | "unsigned" | "unverified" | "unreadable" | "invalid";
  message: string;
  status?: number;
}

/** The browser has no key: it has not been paired, or the pairing was undone. */
export class NoDeviceKeyError extends Error {
  constructor() {
    super("this browser has no key: it has not been paired");
    this.name = "NoDeviceKeyError";
  }
}

/** The header that carries the real method to the pass-through. */
export const METHOD_HEADER = "x-vsms-method";

/** The header that carries the real status back when the pass-through had to
 *  send something else. A sealed `204` still has a body (the message that
 *  signs it), and an HTTP response cannot be a `204` and have one, so the
 *  pass-through answers `200` and names the real status here. It is not
 *  trusted: the status is part of what the seal binds, so a header that lied
 *  about it would make the answer fail to open. */
export const STATUS_HEADER = "x-vsms-status";

/** The only methods the data layer sends. */
const METHODS = new Set(["GET", "POST", "PATCH", "DELETE"]);

/** The request headers a seal binds. Everything else a module sets (`accept`,
 *  `content-type`) is implied by the seal itself, not sent beside it. */
const IDEMPOTENCY_KEY = "idempotency-key";
const IF_MATCH = "if-match";

/** Response headers that may reach the module above: the hint that a retry was
 *  answered from the first, and how long to wait. Neither is covered by the
 *  signature, and neither is a decision. (The `ETag` is not among them: see
 *  {@link etagOf}.) */
const KEPT_RESPONSE_HEADERS = ["idempotency-replayed", "retry-after"] as const;

/** Bodies are never allowed on these. */
const NO_BODY = new Set([101, 204, 205, 304]);

export interface SealingFetchOptions {
  /** The sealing runtime, awaited per call so a browser with no key rejects
   *  here and not at start-up. A {@link NoDeviceKeyError} is "not paired". */
  runtime: () => Promise<SealingRuntime>;
  /** Where the console mounts its pass-through, e.g. `/api/gateway`. */
  endpoint: string;
  /** Posts to the pass-through. Defaults to the global `fetch`; a test passes a
   *  fake gateway here. */
  send?: typeof fetch;
  /** Called once for each 401 this fetch produces or receives: the page that
   *  owns the pairing decides what "pair again" means. */
  onUnauthenticated?: () => void;
}

function errorBody(code: string, message: string): string {
  return JSON.stringify({ code, message });
}

function asSealerFailure(error: unknown): SealerFailure | undefined {
  if (typeof error === "object" && error !== null && "kind" in error && "message" in error) {
    return error as SealerFailure;
  }
  return undefined;
}

/** The status and error body a failure becomes. */
export function refusalOf(failure: SealerFailure): {
  status: number;
  code: string;
  message: string;
} {
  switch (failure.kind) {
    case "unsigned":
      if (failure.status === 401) {
        return {
          status: 401,
          code: "unauthenticated",
          message: "This console is not paired with a vaam admin phone.",
        };
      }
      return unavailable();
    case "unverified":
      return {
        status: 502,
        code: "unverified",
        message: "The answer from the gateway could not be verified, so it was not read.",
      };
    case "unbound":
    case "invalid":
      return { status: 400, code: "invalid_argument", message: failure.message };
    case "seal":
      return {
        status: 401,
        code: "unauthenticated",
        message: "This browser's key could not sign the request. Pair this console again.",
      };
    default:
      return unavailable();
  }
}

function unavailable() {
  return {
    status: 503,
    code: "unavailable",
    message: "The gateway is not reachable. Nothing was read or changed.",
  };
}

function headerValue(init: RequestInit | undefined, name: string): string | undefined {
  const given = init?.headers;
  if (given === undefined) return undefined;
  const value = new Headers(given).get(name);
  return value === null ? undefined : value;
}

/** The `ETag` a write must send back as `If-Match`, read from the SIGNED body.
 *
 *  The gateway stamps a versioned row's `ETag` as `"<version>"` and the row
 *  carries the same `version`, so the validator is rebuilt from the body the
 *  seal covers instead of copied from a header it does not: a header a hop
 *  could have rewritten would otherwise decide which version of a row an
 *  operator overwrites. A body with no integer `version` has no `ETag`. */
export function etagOf(text: string): string | undefined {
  if (!text.startsWith("{")) return undefined;
  try {
    const version = (JSON.parse(text) as { version?: unknown }).version;
    return Number.isSafeInteger(version) ? `"${version}"` : undefined;
  } catch {
    return undefined;
  }
}

/** The sealing `fetch`. */
export function createSealingFetch(options: SealingFetchOptions): GatewayFetch {
  const { runtime, endpoint, onUnauthenticated } = options;
  const send = options.send ?? ((input, init) => globalThis.fetch(input, init));

  const refusal = (failure: SealerFailure): Response => {
    const { status, code, message } = refusalOf(failure);
    if (status === 401) onUnauthenticated?.();
    return new Response(errorBody(code, message), {
      status,
      headers: { "content-type": "application/json" },
    });
  };

  return async (input, init) => {
    const url = new URL(typeof input === "string" ? input : input.toString(), GATEWAY_ORIGIN);
    if (url.origin !== GATEWAY_ORIGIN) {
      return refusal({
        kind: "invalid",
        message: `Not a gateway request: ${url.origin} is not the gateway.`,
      });
    }
    const method = (init?.method ?? "GET").toUpperCase();
    if (!METHODS.has(method)) {
      return refusal({ kind: "invalid", message: `The gateway is not called with ${method}.` });
    }
    const body = init?.body;
    if (body !== undefined && body !== null && typeof body !== "string") {
      return refusal({ kind: "invalid", message: "A sealed request carries JSON text." });
    }
    const query = url.search === "" ? undefined : url.search.slice(1);

    let sealed: WasmSealedCall;
    let sealing: SealingRuntime;
    try {
      sealing = await runtime();
      const { route, params } = sealing.routeOf(method, url.pathname);
      sealed = await sealing.sealer.sealCall(
        method,
        route,
        params,
        body ?? undefined,
        query,
        headerValue(init, IDEMPOTENCY_KEY),
        headerValue(init, IF_MATCH),
      );
    } catch (error) {
      if (error instanceof NoDeviceKeyError)
        return refusal({ kind: "seal", message: error.message });
      return refusal(
        asSealerFailure(error) ?? {
          kind: "seal",
          message: error instanceof Error ? error.message : "The browser's key is not available.",
        },
      );
    }

    // What was signed is what is sent: the sealer built `target` from the
    // parameters it bound, and the module asked for `url`. They must be the
    // same path and query, or the request is dropped before it leaves.
    if (sealed.target !== `${url.pathname}${url.search}`) {
      return refusal({
        kind: "invalid",
        message: "The route the key signed is not the route that was asked for.",
      });
    }

    let answer: Response;
    try {
      answer = await send(`${endpoint}${sealed.target}`, {
        method: "POST",
        credentials: "omit",
        redirect: "error",
        cache: "no-store",
        referrerPolicy: "no-referrer",
        headers: [...sealed.headers, [METHOD_HEADER, method]],
        body: sealed.body as BodyInit,
        ...(init?.signal ? { signal: init.signal } : {}),
      });
    } catch (error) {
      if (init?.signal?.aborted) throw error;
      return refusal({ kind: "unreadable", message: "The console could not be reached." });
    }

    const bytes = new Uint8Array(await answer.arrayBuffer());
    const headers: [string, string][] = [...answer.headers.entries()];
    const claimed = Number(answer.headers.get(STATUS_HEADER));
    const status =
      Number.isInteger(claimed) && claimed >= 200 && claimed < 600 ? claimed : answer.status;
    let text: string;
    try {
      text = await sealed.openResponse(status, headers, bytes);
    } catch (error) {
      return refusal(
        asSealerFailure(error) ?? { kind: "unreadable", message: "The answer could not be read." },
      );
    }

    if (status === 401) onUnauthenticated?.();
    const responseHeaders = new Headers({ "content-type": "application/json" });
    for (const name of KEPT_RESPONSE_HEADERS) {
      const value = answer.headers.get(name);
      if (value !== null) responseHeaders.set(name, value);
    }
    if (status >= 200 && status < 300) {
      const etag = etagOf(text);
      if (etag !== undefined) responseHeaders.set("etag", etag);
    }
    return new Response(NO_BODY.has(status) ? null : text, {
      status,
      headers: responseHeaders,
    });
  };
}
