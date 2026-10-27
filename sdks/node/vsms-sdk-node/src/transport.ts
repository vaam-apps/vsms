/**
 * The HTTP leg of a sealed call, on `node:http` and `node:https`.
 *
 * Not `fetch`: a sealed `GET` carries the signed message as its **body** (the
 * request is a COSE_Sign1 even when the operation has no payload, which is how
 * the signature reaches the gateway), and `fetch` refuses a body on `GET`. The
 * Rust SDK's `reqwest` has no such rule, and neither does `http.request`.
 *
 * One attempt, no retry, no redirect following: a redirect would re-send a
 * signed body (the recipient and the text of a message) to a host the
 * signature was not addressed to. `http.request` never follows one, and a
 * redirect status is simply an answer that is not sealed.
 */
import type { Agent as HttpAgent } from "node:http";
import { request as httpRequest } from "node:http";
import type { Agent as HttpsAgent } from "node:https";
import { request as httpsRequest } from "node:https";
import { SdkError } from "./error.js";
import type { HeaderPairs } from "./wasm.js";

/**
 * The most of an answer's body this client reads, in bytes. A sealed answer is
 * a COSE message around JSON; the largest honest one is a page of messages,
 * a few hundred kilobytes. Eight mebibytes is far above that and far below
 * what a hostile peer could otherwise make this process hold. The same number
 * as `vsms-sdk-rust`'s ceiling on a buffered answer.
 */
export const MAX_RESPONSE_BYTES = 8 * 1024 * 1024;

export interface TransportRequest {
  method: string;
  url: string;
  headers: HeaderPairs;
  body: Uint8Array;
  timeoutMs: number;
}

export interface TransportResponse {
  status: number;
  /** Every response header, in order, duplicates preserved. */
  headers: HeaderPairs;
  body: Uint8Array;
}

/** What {@link sendSealed} needs from the caller's environment. */
export interface TransportOptions {
  /** A custom agent (a proxy, a pool). Defaults to the global agent. */
  agent?: HttpAgent | HttpsAgent | undefined;
}

/** Sends one request and reads the whole answer. Rejects with a `transport` or `response_too_large` {@link SdkError}. */
export function sendSealed(
  request: TransportRequest,
  options: TransportOptions = {},
): Promise<TransportResponse> {
  return new Promise<TransportResponse>((resolve, reject) => {
    const signal = AbortSignal.timeout(request.timeoutMs);
    // A request that outlives its timeout is aborted, and `http.request` then
    // reports a generic `AbortError`. The reason on the signal is the
    // `TimeoutError` a caller can tell from a connection failure, so that is
    // what is attached as the cause.
    const fail = (err: unknown): void =>
      reject(
        new SdkError("request failed", {
          kind: "transport",
          cause: signal.aborted ? (signal.reason as unknown) : err,
        }),
      );

    let url: URL;
    try {
      url = new URL(request.url);
    } catch (err) {
      fail(err);
      return;
    }
    const send = url.protocol === "https:" ? httpsRequest : httpRequest;
    const headers: Record<string, string | string[]> = {};
    for (const [name, value] of request.headers) {
      const key = name.toLowerCase();
      const existing = headers[key];
      headers[key] =
        existing === undefined
          ? value
          : Array.isArray(existing)
            ? [...existing, value]
            : [existing, value];
    }

    // Explicit, because Node omits the length of a `GET` body unless told (it
    // assumes none), and the sealed message would then be read by the gateway
    // as the start of a second, malformed request.
    headers["content-length"] = String(request.body.byteLength);

    const req = send(
      url,
      { method: request.method, headers, agent: options.agent, signal },
      (res) => {
        const tooLarge = (): void => {
          // Reject first: destroying the response emits `aborted`
          // synchronously, and that handler would otherwise settle the promise
          // with a generic transport failure before the typed one.
          reject(
            new SdkError(`the answer is longer than ${MAX_RESPONSE_BYTES} bytes`, {
              kind: "response_too_large",
              httpStatus: res.statusCode,
            }),
          );
          res.destroy();
        };
        // A declared length over the bound is refused before a byte is read;
        // the count below is for a body that declares none (chunked) or lies.
        const declared = Number(res.headers["content-length"]);
        if (Number.isFinite(declared) && declared > MAX_RESPONSE_BYTES) {
          tooLarge();
          return;
        }
        const chunks: Buffer[] = [];
        let received = 0;
        res.on("data", (chunk: Buffer) => {
          received += chunk.byteLength;
          if (received > MAX_RESPONSE_BYTES) {
            chunks.length = 0;
            tooLarge();
            return;
          }
          chunks.push(chunk);
        });
        res.on("error", fail);
        // `aborted` fires when the peer severs a response mid-body; `end`
        // would then never come, and the caller would wait for the timeout.
        res.on("aborted", () => fail(new Error("response aborted")));
        res.on("end", () => {
          const pairs: HeaderPairs = [];
          for (let i = 0; i + 1 < res.rawHeaders.length; i += 2) {
            pairs.push([res.rawHeaders[i] as string, res.rawHeaders[i + 1] as string]);
          }
          resolve({
            status: res.statusCode ?? 0,
            headers: pairs,
            body: Buffer.concat(chunks),
          });
        });
      },
    );
    req.on("error", fail);
    req.end(request.body);
  });
}
