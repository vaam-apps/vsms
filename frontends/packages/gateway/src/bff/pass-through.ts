import "server-only";

// The console's pass-through to the gateway: a pipe for sealed bytes.
//
// It holds no credential and no session. There is no cookie to read, no token
// to attach, no user to look up: a request is worth something to the gateway
// only if the browser's key signed it, and that is checked there, by the
// gateway, on every request. All this file decides is what bytes may be carried
// and where they may go, as pure functions a test can hold, because "a dumb
// pipe" is a claim that is only true while nothing in it is allowed to be smart:
//
//   * Only a `POST` of a COSE message arrives (`./sealing-fetch.ts` explains why
//     a sealed `GET` is posted); the real method is named in `x-vsms-method` and
//     is one of the four the data layer sends.
//   * Only the headers a seal depends on go up (an allow-list). `Authorization`,
//     `Cookie` and everything else a browser or a proxy might add stay behind,
//     because the day this pipe forwarded one is the day it could be made to
//     carry a credential that is not the key's.
//   * Only a route the schema serves is reachable, and that is an allow-list:
//     the method and path must be an operation of the schema (`serves`, the
//     table `routeOf` signs against), because the generated routes and the
//     procedures are exactly the ones the gateway seals
//     (`EnvelopeMode::Required`). The routes that are not (`docs/architecture.md`
//     §4.0: delivery receipts, health, metrics, the OpenID routes and the login
//     pages) are in no such table and so can never be named here, whatever
//     they are called next. Not an open proxy to the gateway's origin.
//   * Nothing comes back but the answer's body, its status and the headers a
//     seal is opened with. Never a `Set-Cookie`, never a `Location`: a redirect
//     must not be followed with a signed body.

import { STATUS_HEADER } from "../sealing-fetch";

/** The headers of a sealed request that go to the gateway. */
export const FORWARDED_REQUEST_HEADERS: readonly string[] = [
  "content-type",
  "accept",
  "cratestack-contract",
  "cratestack-payload-type",
  "cratestack-payload-accept",
  "x-cratestack-schema-sha",
  "idempotency-key",
  "if-match",
];

/** The headers of the gateway's answer that come back: what opening the seal
 *  needs (`content-type`, `cratestack-*`) and the two hints a caller may show. */
export const FORWARDED_RESPONSE_HEADERS: readonly string[] = [
  "content-type",
  "cratestack-payload-type",
  "cratestack-payload-accept",
  "idempotency-replayed",
  "retry-after",
];

/** The methods the data layer sends. */
export const METHODS: ReadonlySet<string> = new Set(["GET", "POST", "PATCH", "DELETE"]);

/** The media type of a sealed message. */
export const COSE_SIGN1 = 'application/cose; cose-type="cose-sign1"';

/** The most a request may carry (a sealed write is a row, not a file). */
export const MAX_REQUEST_BYTES = 1024 * 1024;
/** The most an answer may carry. */
export const MAX_RESPONSE_BYTES = 8 * 1024 * 1024;

/** A path segment of a route the gateway serves: no dots-only segment, no
 *  separators, nothing that a URL parser could resolve away. */
const SEGMENT = /^[A-Za-z0-9$_.~%-]+$/;

/** The path and query the pass-through will open on the gateway, or `undefined`
 *  for a path that is not shaped like one a schema route can be (more than
 *  three segments, a character a route has none of, a dot segment a URL parser
 *  would resolve away). Whether the route exists is not decided here: that is
 *  the schema's table (`RouteTable`). `rest` is what follows the mount
 *  (`/messages/m1?x=1`).
 *
 *  ```ts ignore
 *  upstreamTarget("/messages/m1?fields=id"); // "/messages/m1?fields=id"
 *  upstreamTarget("/messages/../token"); // undefined
 *  ```
 */
export function upstreamTarget(rest: string): string | undefined {
  const [path, ...queryParts] = rest.split("?");
  if (path === undefined || !path.startsWith("/")) return undefined;
  const segments = path.slice(1).split("/");
  if (segments.length === 0 || segments.length > 3) return undefined;
  for (const segment of segments) {
    if (!SEGMENT.test(segment) || /^\.+$/.test(segment)) return undefined;
    let decoded: string;
    try {
      decoded = decodeURIComponent(segment);
    } catch {
      return undefined;
    }
    // An encoded `/` stays inside its segment (the router matches the raw path
    // and decodes a parameter after), which is how the sealer spells an id that
    // has one; a dot segment or a backslash is what a parser would resolve away.
    if (/^\.+$/.test(decoded) || decoded.includes("\\")) return undefined;
  }
  const query = queryParts.join("?");
  return query === "" ? path : `${path}?${query}`;
}

/** Whether the schema serves `method` at `path` (no query, percent-encoding as
 *  received): the table the sealer signs against, which holds every route the
 *  gateway seals and none it does not. In the console it is the Node SDK's
 *  `routeOf`, over the wasm module's `OP_CONTRACTS`. */
export type RouteTable = (method: string, path: string) => boolean;

/** Exactly the allow-listed headers of `from`, nothing else. */
export function pick(from: Headers, allowed: readonly string[]): [string, string][] {
  const out: [string, string][] = [];
  for (const name of allowed) {
    const value = from.get(name);
    if (value !== null) out.push([name, value]);
  }
  return out;
}

/** A request to the gateway. */
export interface UpstreamRequest {
  method: string;
  /** Path and query, as `upstreamTarget` returned them. */
  target: string;
  headers: [string, string][];
  body: Uint8Array;
}

/** What the gateway answered. */
export interface UpstreamAnswer {
  status: number;
  headers: Headers;
  body: Uint8Array;
}

/** Sends one request to the gateway. Never follows a redirect. */
export type Upstream = (request: UpstreamRequest) => Promise<UpstreamAnswer>;

function refuse(status: number, message: string): Response {
  return new Response(JSON.stringify({ code: "refused", message }), {
    status,
    headers: { "content-type": "application/json", "cache-control": "no-store" },
  });
}

/** Reads a request body, refusing one over `limit` whether or not it said so. */
export async function readBounded(
  request: Request,
  limit: number,
): Promise<Uint8Array | undefined> {
  const declared = Number(request.headers.get("content-length") ?? "0");
  if (Number.isFinite(declared) && declared > limit) return undefined;
  const reader = request.body?.getReader();
  if (reader === undefined) return new Uint8Array();
  const chunks: Uint8Array[] = [];
  let total = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    total += value.byteLength;
    if (total > limit) {
      await reader.cancel();
      return undefined;
    }
    chunks.push(value);
  }
  const body = new Uint8Array(total);
  let offset = 0;
  for (const chunk of chunks) {
    body.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return body;
}

/** The handler of `POST <mount>/<path>`. `mount` is the route's own prefix
 *  (`/api/gateway`). */
export function createPassThrough(options: {
  mount: string;
  upstream: Upstream;
  /** The operations of the schema; anything else is a 404. */
  serves: RouteTable;
}): (request: Request) => Promise<Response> {
  const { mount, upstream, serves } = options;
  return async (request) => {
    if (request.method !== "POST") return refuse(405, "Only a sealed POST is carried.");
    const url = new URL(request.url);
    if (!url.pathname.startsWith(`${mount}/`)) return refuse(404, "Not a gateway route.");

    const method = (request.headers.get("x-vsms-method") ?? "").toUpperCase();
    if (!METHODS.has(method)) return refuse(400, "The method is not one the console sends.");

    const mediaType = (request.headers.get("content-type") ?? "").replace(/\s+/g, "").toLowerCase();
    if (mediaType !== COSE_SIGN1.replace(/\s+/g, "").toLowerCase()) {
      return refuse(415, "Only a sealed message is carried.");
    }

    const target = upstreamTarget(`${url.pathname.slice(mount.length)}${url.search}`);
    if (target === undefined) return refuse(404, "Not a gateway route.");
    if (!serves(method, target.split("?")[0] ?? "")) return refuse(404, "Not a gateway route.");

    const body = await readBounded(request, MAX_REQUEST_BYTES);
    if (body === undefined) return refuse(413, "The sealed message is too large.");

    let answer: UpstreamAnswer;
    try {
      answer = await upstream({
        method,
        target,
        headers: pick(request.headers, FORWARDED_REQUEST_HEADERS),
        body,
      });
    } catch {
      return refuse(502, "The gateway could not be reached.");
    }

    const headers = new Headers(pick(answer.headers, FORWARDED_RESPONSE_HEADERS));
    headers.set("cache-control", "no-store");
    // A sealed 204 still carries the message that signs it, and a response
    // cannot be a 204 and have a body: carry it as a 200 and name the real
    // status, which the seal binds (see `STATUS_HEADER`).
    const bodiless = answer.status === 204 || answer.status === 205 || answer.status === 304;
    if (bodiless) headers.set(STATUS_HEADER, String(answer.status));
    return new Response(answer.body as BodyInit, {
      status: bodiless ? 200 : answer.status,
      headers,
    });
  };
}
