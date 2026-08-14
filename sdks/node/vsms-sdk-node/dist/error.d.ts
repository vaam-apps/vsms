/**
 * The one error type this SDK throws or returns.
 * Wraps REST call errors, OAuth / token failures, and validation errors.
 */
export interface SdkErrorOptions {
    httpStatus?: number | undefined;
    gatewayCode?: string | undefined;
    fieldErrors?: Record<string, string[]> | undefined;
    cause?: unknown;
}
export declare class SdkError extends Error {
    readonly httpStatus: number | undefined;
    readonly gatewayCode: string | undefined;
    readonly fieldErrors: Record<string, string[]> | undefined;
    constructor(message: string, options?: SdkErrorOptions);
    /**
     * `true` for a `409 Conflict` (either a duplicate `clientRef` or an in-flight `Idempotency-Key`).
     */
    isConflict(): boolean;
    /**
     * `true` for the specific `409` `IdempotencyLayer` returns while
     * another request under the *same* `Idempotency-Key` is still being
     * processed (not yet reserved-and-replayable).
     */
    isIdempotencyInFlight(): boolean;
    /**
     * `true` for the `422` `IdempotencyLayer` returns when an
     * `Idempotency-Key` is reused with a request that doesn't match the
     * first one under that key byte-for-byte.
     */
    isIdempotencyKeyConflict(): boolean;
    /**
     * `true` if the final error surfaced from a call was a `401 Unauthorized`
     * after bounded refresh was attempted.
     */
    isUnauthorized(): boolean;
}
//# sourceMappingURL=error.d.ts.map