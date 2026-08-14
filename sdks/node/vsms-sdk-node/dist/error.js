/**
 * The one error type this SDK throws or returns.
 * Wraps REST call errors, OAuth / token failures, and validation errors.
 */
export class SdkError extends Error {
    httpStatus;
    gatewayCode;
    fieldErrors;
    constructor(message, options) {
        super(message, { cause: options?.cause });
        this.name = "SdkError";
        this.httpStatus = options?.httpStatus;
        this.gatewayCode = options?.gatewayCode;
        this.fieldErrors = options?.fieldErrors;
    }
    /**
     * `true` for a `409 Conflict` (either a duplicate `clientRef` or an in-flight `Idempotency-Key`).
     */
    isConflict() {
        return this.httpStatus === 409;
    }
    /**
     * `true` for the specific `409` `IdempotencyLayer` returns while
     * another request under the *same* `Idempotency-Key` is still being
     * processed (not yet reserved-and-replayable).
     */
    isIdempotencyInFlight() {
        return this.httpStatus === 409 && this.message.includes("Idempotency-Key");
    }
    /**
     * `true` for the `422` `IdempotencyLayer` returns when an
     * `Idempotency-Key` is reused with a request that doesn't match the
     * first one under that key byte-for-byte.
     */
    isIdempotencyKeyConflict() {
        return this.httpStatus === 422 && this.message.includes("idempotency_key_conflict");
    }
    /**
     * `true` if the final error surfaced from a call was a `401 Unauthorized`
     * after bounded refresh was attempted.
     */
    isUnauthorized() {
        return this.httpStatus === 401;
    }
}
//# sourceMappingURL=error.js.map