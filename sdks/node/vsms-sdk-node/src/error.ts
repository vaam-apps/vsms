/**
 * The one error type this SDK throws.
 *
 * `kind` says which part of a call failed, so a caller can tell "the gateway
 * said no" from "the answer cannot be believed" without parsing a message:
 *
 * - `config`: an option is missing or unusable (a key, the pinned server keys).
 *   Thrown by the constructor, never by the first request.
 * - `seal`: the call was refused **before anything was sent**: the signer
 *   failed or took longer than `timeoutMs`, a parameter does not fit its route,
 *   or the URL would not be the one that was signed. No request left this
 *   process, so a caller may treat this kind as safe to retry. It is never used
 *   for a failure after the send.
 * - `transport`: the request could not be sent or the answer not received
 *   (a refused connection, a timeout, a severed body).
 * - `response_too_large`: the answer is longer than this client reads.
 * - `unsigned`: the answer is not a sealed message, whatever its status. The
 *   gateway's own refusal of a signature is this kind with `httpStatus` 401.
 * - `unverified`: the answer claims to be sealed and does not verify under a
 *   pinned server key, or does not answer this request.
 * - `unreadable`: the request **was sent**, and the answer arrived but could not
 *   be read (an unexpected payload type, a malformed header, text that is not
 *   UTF-8). Whether the gateway acted on the call is unknown: do not retry a
 *   write without its `Idempotency-Key`.
 * - `remote`: a sealed, verified answer with a non-2xx status.
 * - `decode`: a sealed, verified 2xx answer whose body is not the expected JSON,
 *   or a verified webhook delivery that is not the documented envelope.
 * - `backend`: a webhook delivery could not be checked because the receiver's
 *   own machinery failed (answer `5xx`; vsms retries).
 */
export type SdkErrorKind =
  | "config"
  | "seal"
  | "transport"
  | "response_too_large"
  | "unsigned"
  | "unverified"
  | "unreadable"
  | "remote"
  | "decode"
  | "backend";

export interface SdkErrorOptions {
  kind?: SdkErrorKind | undefined;
  httpStatus?: number | undefined;
  gatewayCode?: string | undefined;
  fieldErrors?: Record<string, string[]> | undefined;
  cause?: unknown;
}

export class SdkError extends Error {
  readonly kind: SdkErrorKind;
  readonly httpStatus: number | undefined;
  readonly gatewayCode: string | undefined;
  readonly fieldErrors: Record<string, string[]> | undefined;

  constructor(message: string, options?: SdkErrorOptions) {
    super(message, { cause: options?.cause });
    this.name = "SdkError";
    this.kind = options?.kind ?? "remote";
    this.httpStatus = options?.httpStatus;
    this.gatewayCode = options?.gatewayCode;
    this.fieldErrors = options?.fieldErrors;
  }

  /**
   * `true` for a `409 Conflict` (either a duplicate `clientRef` or an in-flight `Idempotency-Key`).
   */
  isConflict(): boolean {
    return this.httpStatus === 409;
  }

  /**
   * `true` for the specific `409` `IdempotencyLayer` returns while
   * another request under the *same* `Idempotency-Key` is still being
   * processed (not yet reserved-and-replayable).
   */
  isIdempotencyInFlight(): boolean {
    return this.httpStatus === 409 && this.message.includes("Idempotency-Key");
  }

  /**
   * `true` for the `422` `IdempotencyLayer` returns when an
   * `Idempotency-Key` is reused with a request that doesn't match the
   * first one under that key byte-for-byte.
   */
  isIdempotencyKeyConflict(): boolean {
    return this.httpStatus === 422 && this.message.includes("idempotency_key_conflict");
  }

  /**
   * `true` when the gateway did not accept the signature: the unsigned `401`
   * it answers to an unknown or revoked key, an inactive client, a stale
   * `iat` (check this machine's clock: the gateway allows five minutes), a
   * replayed request or a bad signature. It is deliberately one answer for
   * all of them, so the SDK can say no more; the first things to check are
   * that the key's public half is registered and not revoked, and that the
   * clock is right. The SDK does **not** retry it: a retry is a new request
   * with a new nonce, the caller's decision.
   */
  isUnauthorized(): boolean {
    return this.httpStatus === 401;
  }
}
