import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { verifySignature } from "./signature.ts";

/**
 * #41's cross-language proof, the Node half.
 *
 * `backends/crates/sms-webhook/tests/fixtures/cross_language_vectors.json` is a
 * fixture shared with `backends/crates/sms-webhook/tests/cross_language_fixtures.rs`
 * (the Rust side of the same proof — see that file's own module doc).
 * Every `signatureHeader` value in it was computed with neither this
 * file's code nor the Rust crate's — a third, independent tool
 * (`openssl dgst -sha256 -hmac`, receipts in the fixture's own
 * `$comment`) — so `verifySignature` (this file's own from-scratch
 * transcription of §4.4, written *before* `sms-webhook` existed — see
 * `signature.ts`'s module doc) agreeing with all of them is what turns
 * that file's "one genuine guess" (the MAC algorithm) into a confirmed
 * fact rather than a documented assumption.
 *
 * Run directly with:
 *
 * ```bash
 * cd examples/node/webhook-receiver && pnpm install && node --test src
 * ```
 */

const here = dirname(fileURLToPath(import.meta.url));
const fixturePath = join(
  here,
  "..",
  "..",
  "..",
  "..",
  "crates",
  "sms-webhook",
  "tests",
  "fixtures",
  "cross_language_vectors.json",
);

interface Vector {
  name: string;
  secrets: string[];
  timestampUnix: number;
  eventId: string;
  bodyUtf8: string;
  signatureHeader: string;
  expectVerifies: boolean;
}

function loadVectors(): Vector[] {
  const raw = readFileSync(fixturePath, "utf8");
  const parsed = JSON.parse(raw) as { vectors: Vector[] };
  return parsed.vectors;
}

const EXPECTED_VECTOR_NAMES = [
  "signed-with-current-secret",
  "signed-with-prev-secret-during-rotation",
  "header-carries-both-values-oldest-last",
  "tampered-body-fails",
  "wrong-secret-fails",
  "malformed-signature-header-fails",
];

test("the fixture file still has every expected vector", () => {
  const names = loadVectors().map((vector) => vector.name);
  for (const expected of EXPECTED_VECTOR_NAMES) {
    assert.ok(
      names.includes(expected),
      `expected vector "${expected}" is missing from the fixture; names present: ${JSON.stringify(names)}`,
    );
  }
});

test("verifySignature agrees with every Rust-computed fixture vector", () => {
  const vectors = loadVectors();
  assert.ok(vectors.length > 0, "the fixture file must not be empty");

  for (const vector of vectors) {
    const result = verifySignature({
      rawBody: Buffer.from(vector.bodyUtf8, "utf8"),
      timestamp: String(vector.timestampUnix),
      eventId: vector.eventId,
      signatureHeader: vector.signatureHeader,
      secrets: vector.secrets,
    });
    assert.equal(
      result.ok,
      vector.expectVerifies,
      `vector "${vector.name}": expected ok=${vector.expectVerifies}, got ${JSON.stringify(result)}`,
    );
  }
});
