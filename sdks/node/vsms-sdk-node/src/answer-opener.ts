/**
 * Believing the gateway's answer to a request this process did not seal.
 *
 * A `VsmsClient` seals its own calls and opens their answers with the state
 * the seal kept. A server that only forwards another party's sealed request
 * (the operator console's composer route forwards a browser's) has no such
 * state and still must not take a `200` from a socket for the gateway's word:
 * it opens the answer here, against the very bytes it forwarded, under the two
 * pinned server keys. The check is the one `SealedCall.openResponse` makes
 * (cratestack's envelope, through the same wasm module), not a copy of it.
 */
import { SdkError } from "./error.js";
import { toKeyBase64url, type ServerKeysOption } from "./identity.js";
import { answerFailure } from "./sealed-call.js";
import { type HeaderPairs, isWasmFailure, loadWasm, type WasmOpener } from "./wasm.js";

export interface AnswerOpenerOptions {
  /** The gateway's pinned response keys, as for a `VsmsClient`. */
  serverKeys: ServerKeysOption;
}

/** The request that was forwarded. */
export interface ForwardedRequest {
  method: string;
  /** The path and query it was sent to: `/$procs/authorizeComposition`. */
  target: string;
  /** The sealed message, exactly as sent. */
  body: Uint8Array;
  /** The headers it was sent with. Only `Idempotency-Key` and `If-Match` are read. */
  headers?: HeaderPairs | undefined;
}

/** What came back. */
export interface ForwardedAnswer {
  status: number;
  headers: HeaderPairs;
  body: Uint8Array;
}

export class AnswerOpener {
  readonly #opener: WasmOpener;

  constructor(options: AnswerOpenerOptions) {
    if (options.serverKeys === undefined || options.serverKeys === null) {
      throw new SdkError(
        "serverKeys is required: the opener refuses to trust an answer it has no key for",
        { kind: "config" },
      );
    }
    const current = toKeyBase64url(options.serverKeys.current, "serverKeys.current");
    const next = toKeyBase64url(options.serverKeys.next, "serverKeys.next");
    try {
      this.#opener = loadWasm().Opener.forServerKeys(current, next);
    } catch (err) {
      if (err instanceof SdkError) throw err;
      throw new SdkError(
        isWasmFailure(err) ? err.message : "the server keys could not be used",
        { kind: "config", cause: err },
      );
    }
  }

  /**
   * The verified JSON text of `answer`. Rejects with an {@link SdkError} of
   * kind `unsigned` (not a sealed message, whatever its status: the gateway's
   * coarse `401` is this), `unverified` (sealed, but not by a pinned key, or
   * not as the answer to this request and this status) or `unreadable`.
   */
  async open(request: ForwardedRequest, answer: ForwardedAnswer): Promise<string> {
    const [path = "", query] = splitTarget(request.target);
    try {
      return await this.#opener.openAnswer(
        request.method,
        path,
        query,
        request.body,
        request.headers ?? [],
        answer.status,
        answer.headers,
        answer.body,
      );
    } catch (err) {
      throw answerFailure(err, answer.status);
    }
  }
}

function splitTarget(target: string): [string, string | undefined] {
  const at = target.indexOf("?");
  return at < 0 ? [target, undefined] : [target.slice(0, at), target.slice(at + 1)];
}
