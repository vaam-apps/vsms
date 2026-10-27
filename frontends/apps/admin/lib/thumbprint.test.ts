// The thumbprint is RFC 9679's, not a hash of the raw point. It is rebuilt here
// from the CBOR map's bytes written out one by one, which is what the gateway's
// own unit test does (`backends/crates/sms-api/src/console_keys.rs`,
// `thumbprint_by_hand`), and compared against Node's SHA-256.

import { createHash } from "node:crypto";
import { describe, expect, it } from "vitest";
import { loadOrCreateDeviceKey, memoryKeyStore } from "./device-key";
import { coseThumbprint } from "./thumbprint";

describe("coseThumbprint", () => {
  it("is SHA-256 of {1: 2, -1: 1, -2: x, -3: y} in deterministic CBOR, as lowercase hex", async () => {
    const { publicKey } = await loadOrCreateDeviceKey(memoryKeyStore());
    const x = Buffer.from(publicKey.subarray(1, 33));
    const y = Buffer.from(publicKey.subarray(33, 65));
    // a4 | 01 02 | 20 01 | 21 58 20 x | 22 58 20 y
    const map = Buffer.concat([
      Buffer.from([0xa4, 0x01, 0x02, 0x20, 0x01, 0x21, 0x58, 0x20]),
      x,
      Buffer.from([0x22, 0x58, 0x20]),
      y,
    ]);

    const thumbprint = await coseThumbprint(publicKey);

    expect(thumbprint).toBe(createHash("sha256").update(map).digest("hex"));
    expect(thumbprint).toMatch(/^[0-9a-f]{64}$/);
  });

  it("is not a hash of the raw point", async () => {
    const { publicKey } = await loadOrCreateDeviceKey(memoryKeyStore());

    expect(await coseThumbprint(publicKey)).not.toBe(
      createHash("sha256").update(publicKey).digest("hex"),
    );
  });

  it("refuses a compressed point: one canonical spelling", async () => {
    await expect(coseThumbprint(new Uint8Array(33))).rejects.toThrow(/uncompressed/);
  });
});
