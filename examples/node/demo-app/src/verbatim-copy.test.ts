import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

/**
 * `signature.ts` (and its own `cross-language-vectors.test.ts`) in this
 * package are deliberate, byte-for-byte copies of
 * `examples/node/webhook-receiver/src/signature.ts` — not a
 * re-derivation, per this package's own README. A hand-copy with no
 * automatic check is exactly how two implementations of the same
 * signing scheme silently drift the moment one file is edited and the
 * other isn't — `signature.ts`'s own module doc already tells that
 * story once, for the gap between "an obvious guess" and "confirmed
 * against a third, independent tool." This test closes the second,
 * narrower gap it doesn't cover: two *copies* of the same confirmed
 * file disagreeing with each other. It fails loudly, naming both paths,
 * the moment they do.
 */
const here = dirname(fileURLToPath(import.meta.url));
const sourceOfTruth = join(here, "..", "..", "webhook-receiver", "src");

for (const file of ["signature.ts", "cross-language-vectors.test.ts"]) {
  test(`${file} is byte-for-byte identical to examples/node/webhook-receiver/src/${file}`, () => {
    const thisCopy = readFileSync(join(here, file), "utf8");
    const original = readFileSync(join(sourceOfTruth, file), "utf8");
    assert.equal(
      thisCopy,
      original,
      `examples/node/demo-app/src/${file} has drifted from examples/node/webhook-receiver/src/${file} — ` +
        "copy the webhook-receiver file over this one verbatim again.",
    );
  });
}
