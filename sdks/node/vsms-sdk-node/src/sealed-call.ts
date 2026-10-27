/**
 * One sealed call, start to finish: seal, send, open.
 *
 * The single place every `VsmsClient` method goes through, so that all of them
 * bind the same bytes and map the same failures to the same errors.
 */
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
  /** `baseUrl` with any trailing slash removed. */
  baseUrl: string;
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

function mapFailure(err: unknown, status?: number): SdkError {
  if (!isWasmFailure(err)) {
    return new SdkError("sealing the call failed", { kind: "seal", cause: err });
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
      return new SdkError(err.message, { kind: "seal", httpStatus: status });
  }
}

/**
 * Seals `spec`, sends it, and opens the answer. Exactly one attempt: a sealed
 * request is never retried here (a retry is a new message with a new nonce,
 * which is only safe for a call whose `Idempotency-Key` the caller chose).
 * An answer that is not sealed, or does not verify, is an error whatever its
 * status, and nothing in its body is read.
 */
export async function performSealedCall(
  context: CallContext,
  spec: SealedCallSpec,
): Promise<OpenedAnswer> {
  let sealed: WasmSealedCall;
  try {
    sealed = await context.sealer.sealCall(
      spec.method,
      spec.route,
      spec.params ?? [],
      spec.body,
      spec.query,
      spec.idempotencyKey,
    );
  } catch (err) {
    throw mapFailure(err);
  }

  const response = await sendSealed(
    {
      method: spec.method,
      // The target is built by the sealing module from the parameters it bound.
      url: `${context.baseUrl}${sealed.target}`,
      headers: sealed.headers,
      body: sealed.body,
      timeoutMs: context.timeoutMs,
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
    throw mapFailure(err, response.status);
  }
}
