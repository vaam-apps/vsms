import { strict as assert } from "node:assert";
import { test } from "node:test";
import * as sdk from "../dist/index.js";
import { SdkError } from "../dist/index.js";

test("SdkError classification methods work correctly", () => {
  const genericErr = new SdkError("something went wrong", { httpStatus: 500 });
  assert.equal(genericErr.isConflict(), false);
  assert.equal(genericErr.isIdempotencyInFlight(), false);
  assert.equal(genericErr.isIdempotencyKeyConflict(), false);
  assert.equal(genericErr.isUnauthorized(), false);

  const conflictErr = new SdkError("duplicate key", { httpStatus: 409 });
  assert.equal(conflictErr.isConflict(), true);
  assert.equal(conflictErr.isIdempotencyInFlight(), false);

  const inFlightErr = new SdkError("another request with this Idempotency-Key is still in flight", {
    httpStatus: 409,
  });
  assert.equal(inFlightErr.isConflict(), true);
  assert.equal(inFlightErr.isIdempotencyInFlight(), true);

  const keyConflictErr = new SdkError("idempotency_key_conflict: payload mismatch", {
    httpStatus: 422,
  });
  assert.equal(keyConflictErr.isIdempotencyKeyConflict(), true);

  const unauthErr = new SdkError("unauthorized", { httpStatus: 401 });
  assert.equal(unauthErr.isUnauthorized(), true);
});

test("the package exports a client and its errors and no token machinery", () => {
  // The removed token protocol had a token store, a private_key_jwt client
  // factory and a `tokenStore` accessor. There is no compatibility mode: not
  // one of them may come back as an export.
  assert.deepEqual(Object.keys(sdk).sort(), [
    "AnswerOpener",
    "DEFAULT_TIMEOUT_MS",
    "MAX_RESPONSE_BYTES",
    "SdkError",
    "VsmsClient",
    "WebhookOpener",
    "parseServerKeysReport",
    "routeOf",
  ]);
  assert.equal("privateKeyJwt" in sdk.VsmsClient, false);
  assert.equal("tokenStore" in sdk.VsmsClient.prototype, false);
});
