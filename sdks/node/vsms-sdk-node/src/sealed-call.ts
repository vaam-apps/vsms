/**
 * One sealed call, start to finish: seal, send, open.
 *
 * The single place every `VsmsClient` method goes through, so that all of them
 * bind the same bytes and map the same failures to the same errors.
 */
import { type GatewayBase, sealedUrl } from "./base-url.js";
import { SdkError } from "./error.js";
import { sendSealed, type TransportOptions } from "./transport.js";
import { isWasmFailure, type WasmSealedCall, type WasmSealer } from "./wasm.js";

export interface SealedCallSpec {
  method: "GET" | "POST";
  /** The schema's route template: `/messages/{id}`, `/$procs/sendMessage`. */
  route: string;
  /** The decoded values of the template's `{parameters}`, in order. */
  params?: string[] | undefined;
  /** The JSON text of a write. Absent for a read. */
  body?: string | undefined;
  /** The query string as it will be sent, without the `?`. */
  query?: string | undefined;
  /** The `Idempotency-Key` of a write, bound by the signature as sent. */
  idempotencyKey?: string | undefined;
}

/** What a call needs from the client that makes it. */
export interface CallContext extends TransportOptions {
  /** The gateway's parsed address. */
  base: GatewayBase;
  /** One deadline for the whole call: signing, sending and reading the answer. */
  timeoutMs: number;
  sealer: WasmSealer;
}

/** An answer that verified. */
export interface OpenedAnswer {
  /** The HTTP status, which the signature also binds. */
  status: number;
  /** The verified JSON text. */
  text: string;
  /**
   * The response headers as they arrived, lower-cased, last one winning.
   * **Not authenticated**: the signature covers the payload, the status and the
   * request it answers, not these. Good for the `Idempotency-Replayed` hint and
   * a request id in a log line, nothing a decision should rest on.
   */
  headers: Record<string, string>;
}

/**
 * A failure before anything was sent. Always `seal`: a caller may treat that
 * kind as "no request left this process" (and, say, retry), which is why no
 * failure that happens after the send is ever mapped here.
 */
function sealFailure(err: unknown): SdkError {
  if (err instanceof SdkError) return err;
  if (!isWasmFailure(err)) {
    return new SdkError("sealing the call failed", { kind: "seal", cause: err });
  }
  return new SdkError(err.message, { kind: "seal" });
}

/**
 * A failure once the request was sent: the answer is not believed
 * (`unsigned`, `unverified`) or cannot be read (`unreadable`). Never `seal`,
 * since the request was delivered and a retry would send it twice.
 */
export function answerFailure(err: unknown, status: number): SdkError {
  if (!isWasmFailure(err)) {
    return new SdkError("the answer could not be read", {
      kind: "unreadable",
      httpStatus: status,
      cause: err,
    });
  }
  switch (err.kind) {
    case "unsigned":
      return new SdkError(err.message, {
        kind: "unsigned",
        httpStatus: err.status ?? status,
      });
    case "unverified":
      return new SdkError(err.message, { kind: "unverified", httpStatus: status });
    default:
      return new SdkError(err.message, { kind: "unreadable", httpStatus: status });
  }
}

/**
 * `work`, or a `seal` error if it has not settled within `ms`. The signing
 * callback is the caller's code (a KMS, an HSM) and may never answer; without
 * this the call would hang past `timeoutMs`, which is the one bound a caller
 * set. `work` is left running when the deadline wins: whatever it settles to
 * later is nobody's.
 */
async function withinDeadline<T>(work: Promise<T>, ms: number): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const late = new Promise<never>((_, reject) => {
    timer = setTimeout(
      () =>
        reject(
          new SdkError(`signing the call took longer than ${ms} ms`, {
            kind: "seal",
            cause: new DOMException("The operation timed out.", "TimeoutError"),
          }),
        ),
      ms,
    );
  });
  work.catch(() => undefined);
  try {
    return await Promise.race([work, late]);
  } finally {
    clearTimeout(timer);
  }
}

/**
 * Seals `spec`, sends it, and opens the answer. Exactly one attempt: a sealed
 * request is never retried here (a retry is a new message with a new nonce,
 * which is only safe for a call whose `Idempotency-Key` the caller chose).
 * An answer that is not sealed, or does not verify, is an error whatever its
 * status, and nothing in its body is read. One deadline, `timeoutMs`, covers
 * the signing as well as the request.
 */
export async function performSealedCall(
  context: CallContext,
  spec: SealedCallSpec,
): Promise<OpenedAnswer> {
  const startedAt = performance.now();
  let sealed: WasmSealedCall;
  let url: string;
  try {
    sealed = await withinDeadline(
      Promise.resolve().then(() =>
        context.sealer.sealCall(
          spec.method,
          spec.route,
          spec.params ?? [],
          spec.body,
          spec.query,
          spec.idempotencyKey,
        ),
      ),
      context.timeoutMs,
    );
    // The target is built by the sealing module from the parameters it bound;
    // what is checked here is that the URL says the same.
    url = sealedUrl(context.base, sealed.target);
  } catch (err) {
    throw sealFailure(err);
  }

  const response = await sendSealed(
    {
      method: spec.method,
      url,
      headers: sealed.headers,
      body: sealed.body,
      // What is left of the one deadline, never more than the whole of it.
      timeoutMs: Math.max(1, Math.ceil(context.timeoutMs - (performance.now() - startedAt))),
    },
    { agent: context.agent },
  );

  try {
    const text = await sealed.openResponse(response.status, response.headers, response.body);
    const headers: Record<string, string> = {};
    for (const [name, value] of response.headers) {
      headers[name.toLowerCase()] = value;
    }
    return { status: response.status, text, headers };
  } catch (err) {
    throw answerFailure(err, response.status);
  }
}
