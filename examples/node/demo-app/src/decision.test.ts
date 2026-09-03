import assert from "node:assert/strict";
import { test } from "node:test";
import { decide } from "./decision.ts";

/**
 * All four (delivered × verified) quadrants, asserted directly against
 * `decide`'s own output — no server, no HTTP, no SDK. This is the test
 * that a sabotaged predicate (`delivered || verifiedCount === 0`, the
 * exact inversion found live against the previous inline version of
 * this logic) cannot pass: it flips exactly the cases below.
 */

test("delivered with at least one verified webhook succeeds", () => {
  const result = decide({ delivered: true, verifiedCount: 1, eventCount: 1 });
  assert.equal(result.exitCode, 0);
  assert.deepEqual(result.reasons, []);
});

test("delivered with zero verified webhooks fails", () => {
  const result = decide({ delivered: true, verifiedCount: 0, eventCount: 3 });
  assert.equal(result.exitCode, 1);
  assert.deepEqual(result.reasons, [
    "3 webhook(s) were received but NONE verified their signature — check that WEBHOOK secret matches the seeded WebhookEndpoint",
  ]);
});

test("not delivered with verified webhooks still fails", () => {
  const result = decide({ delivered: false, verifiedCount: 2, eventCount: 2 });
  assert.equal(result.exitCode, 1);
  assert.deepEqual(result.reasons, ["message never reached delivered"]);
});

test("neither delivered nor verified fails, with both reasons", () => {
  const result = decide({ delivered: false, verifiedCount: 0, eventCount: 0 });
  assert.equal(result.exitCode, 1);
  assert.deepEqual(result.reasons, [
    "message never reached delivered",
    "no webhook was received at all",
  ]);
});

test("zero received webhooks and zero verified reports 'none received', not a bogus count", () => {
  const result = decide({ delivered: true, verifiedCount: 0, eventCount: 0 });
  assert.equal(result.exitCode, 1);
  assert.deepEqual(result.reasons, ["no webhook was received at all"]);
});
