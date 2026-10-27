/**
 * A real `node:http` server for tests, per the SDK task's decisive-tests
 * requirement: assert the raw bytes on the wire, never a mocked `fetch`.
 *
 * Not reachable from a shipping process: this directory is outside `src`, so
 * it is in neither `dist` nor the package's `files`.
 */
import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import type { AddressInfo } from "node:net";
import type { HeaderPairs } from "../src/wasm.ts";
import {
  loadResponderModule,
  newTestIdentity,
  type TestIdentity,
  type WasmResponder,
} from "./responder.ts";

export interface RecordedRequest {
  method: string;
  url: string;
  headers: IncomingMessage["headers"];
  body: Buffer;
}

export interface TestServer {
  url: string;
  requests: RecordedRequest[];
  close(): Promise<void>;
}

export type TestHandler = (request: RecordedRequest, response: ServerResponse) => void;

function listen(server: ReturnType<typeof createServer>): Promise<string> {
  return new Promise((resolve, reject) => {
    // Rejects the startup promise if `listen` fails; detached once listening so
    // a later `error` event is not swallowed by an already-settled promise.
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      server.off("error", reject);
      resolve(`http://127.0.0.1:${(server.address() as AddressInfo).port}`);
    });
  });
}

function closer(server: ReturnType<typeof createServer>): () => Promise<void> {
  return () =>
    new Promise<void>((resolve, reject) => {
      // `close` alone waits for every open connection to end, which never
      // happens for a deliberately stalled response.
      server.closeAllConnections();
      server.close((err) => (err ? reject(err) : resolve()));
    });
}

/** Starts a plain HTTP server on an ephemeral loopback port, recording every request. */
export async function startTestServer(handler: TestHandler): Promise<TestServer> {
  const requests: RecordedRequest[] = [];
  const server = createServer((req, res) => {
    const chunks: Buffer[] = [];
    req.on("data", (chunk: Buffer) => chunks.push(chunk));
    req.on("end", () => {
      const record: RecordedRequest = {
        method: req.method ?? "",
        url: req.url ?? "",
        headers: req.headers,
        body: Buffer.concat(chunks),
      };
      requests.push(record);
      handler(record, res);
    });
  });
  const url = await listen(server);
  return { url, requests, close: closer(server) };
}

// ---------------------------------------------------------------- sealed

/** A request the gateway would have accepted: verified, and read back plain. */
export interface OpenedRecord {
  method: string;
  url: string;
  route: string;
  params: string[];
  payload: string;
  idempotencyKey: string | undefined;
  headers: IncomingMessage["headers"];
}

/** What a handler answers a verified request with; the server seals it. */
export interface Reply {
  status: number;
  body: string;
  /** Extra headers, outside the seal (so: only hints). */
  headers?: Record<string, string>;
}

export type SealedHandler = (request: OpenedRecord) => Reply;

export interface SealedServerOptions {
  identity?: TestIdentity;
  /** The audience the server expects. Default `vsms`, the gateway's. */
  audience?: string;
  /** Seals answers under this seed instead of the identity's current one. */
  answerSeed?: string;
  /** Registers no client key at all, so every request is refused unsigned. */
  registerClient?: boolean;
}

export interface SealedTestServer extends TestServer {
  identity: TestIdentity;
  /** What a client needs to talk to this server. */
  clientOptions: {
    baseUrl: string;
    privateKey: string;
    serverKeys: { current: string; next: string };
  };
  /** Requests that verified and reached the handler. */
  opened: OpenedRecord[];
}

function pairsOf(raw: IncomingMessage["rawHeaders"]): HeaderPairs {
  const out: HeaderPairs = [];
  for (let i = 0; i + 1 < raw.length; i += 2) {
    out.push([raw[i] as string, raw[i + 1] as string]);
  }
  return out;
}

/**
 * A `node:http` server that behaves like the gateway for sealed callers: it
 * opens each request under the client's registered key, hands the handler the
 * plain request, and seals what the handler answers under the server's key. A
 * request that does not verify, or that repeats one that did, gets the unsigned
 * coarse `401` and never reaches the handler, as it would not at the gateway.
 */
export async function startSealedServer(
  handler: SealedHandler,
  options: SealedServerOptions = {},
): Promise<SealedTestServer> {
  const identity = options.identity ?? newTestIdentity();
  const wasm = loadResponderModule();
  const requests: RecordedRequest[] = [];
  const opened: OpenedRecord[] = [];
  const responder: WasmResponder = new wasm.Responder(
    options.answerSeed ?? identity.serverSeed,
    options.registerClient === false ? [] : [identity.clientPublic],
    options.audience ?? "vsms",
  );

  const server = createServer((req, res) => {
    const chunks: Buffer[] = [];
    req.on("data", (chunk: Buffer) => chunks.push(chunk));
    req.on("end", () => {
      const body = Buffer.concat(chunks);
      requests.push({
        method: req.method ?? "",
        url: req.url ?? "",
        headers: req.headers,
        body,
      });
      const url = new URL(req.url ?? "/", "http://placeholder.invalid");
      const query = url.search.length > 1 ? url.search.slice(1) : undefined;
      responder
        .openRequest(req.method ?? "", url.pathname, query, pairsOf(req.rawHeaders), body)
        .then(
          (request) => {
            const record: OpenedRecord = {
              method: req.method ?? "",
              url: req.url ?? "",
              route: request.route,
              params: request.pathParams,
              payload: request.payload,
              idempotencyKey: request.idempotencyKey,
              headers: req.headers,
            };
            opened.push(record);
            const reply = handler(record);
            return request.sealResponse(reply.status, reply.body).then((sealed) => {
              const headers: Record<string, string> = { ...reply.headers };
              for (const [name, value] of sealed.headers) headers[name] = value;
              res.writeHead(reply.status, headers);
              res.end(Buffer.from(sealed.body));
            });
          },
          () => {
            // The gateway's refusal: one coarse, unsigned 401.
            res.writeHead(401, { "content-type": "application/json" });
            res.end(
              JSON.stringify({
                code: "UNAUTHORIZED",
                message: "request could not be authenticated",
              }),
            );
          },
        )
        .catch(() => {
          res.writeHead(500);
          res.end();
        });
    });
  });
  const url = await listen(server);

  return {
    url,
    requests,
    opened,
    identity,
    clientOptions: {
      baseUrl: url,
      privateKey: identity.clientSeed,
      serverKeys: { current: identity.serverPublic, next: identity.nextPublic },
    },
    close: closer(server),
  };
}
