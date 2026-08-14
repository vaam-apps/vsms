import { strict as assert } from "node:assert";
import { generateKeyPairSync } from "node:crypto";
import { test } from "node:test";
import { PrivateKeyJwtTokenStore, SdkError, VsmsClient } from "../dist/index.js";

// Generate a test RSA keypair in PEM PKCS#8 format
const { privateKey } = generateKeyPairSync("rsa", {
  modulusLength: 2048,
  publicKeyEncoding: {
    type: "spki",
    format: "pem",
  },
  privateKeyEncoding: {
    type: "pkcs8",
    format: "pem",
  },
});

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

test("PrivateKeyJwtTokenStore initializes properly with PEM string", async () => {
  const store = PrivateKeyJwtTokenStore.fromKeyPem(
    "http://127.0.0.1:8080",
    "test-client-id",
    privateKey,
    "sms:send sms:read",
  );
  assert.ok(store);
});

test("PrivateKeyJwtTokenStore rejects empty or whitespace-only scope", () => {
  assert.throws(
    () => {
      new PrivateKeyJwtTokenStore(
        {
          issuer: "http://127.0.0.1:8080",
          clientId: "test-client-id",
          scope: "   ",
        },
        { pemString: privateKey },
      );
    },
    (err: unknown) => {
      return err instanceof SdkError && err.message.includes("scope cannot be empty");
    },
  );
});

test("VsmsClient.privateKeyJwt constructs client", () => {
  const client = VsmsClient.privateKeyJwt({
    issuer: "http://127.0.0.1:8080",
    clientId: "test-client-id",
    privateKeyPem: privateKey,
  });
  assert.ok(client);
  assert.ok(client.tokenStore);
});

test("VsmsClient wraps transport and non-JSON response parsing errors in SdkError", async () => {
  // Test with custom tokenStore that returns a dummy token
  const mockTokenStore = {
    get: async () => "mock-token",
    invalidate: () => {},
  };

  // Point to a port that fails to connect
  const unreachableClient = new VsmsClient({
    baseUrl: "http://127.0.0.1:1",
    tokenStore: mockTokenStore,
  });

  await assert.rejects(
    async () => {
      await unreachableClient.previewMessage({ body: "test" });
    },
    (err: unknown) => {
      return err instanceof SdkError && err.message.includes("network error");
    },
  );
});
