// The browser's key: made by WebCrypto as non-extractable, stored as the
// `CryptoKey` object in IndexedDB, and gone when unpaired. The property that
// matters most is that nothing in the page, this module included, can read the
// private half, so that is asserted against the key itself and against the copy
// that comes back out of storage, not against a flag this module sets.

import "fake-indexeddb/auto";
import { beforeEach, describe, expect, it } from "vitest";
import {
  deleteDeviceKey,
  generatePair,
  indexedDbKeyStore,
  loadDeviceKey,
  loadOrCreateDeviceKey,
  markPaired,
  memoryKeyStore,
} from "./device-key";

const SIGN = { name: "ECDSA", hash: "SHA-256" } as const;

const verifies = (key: CryptoKey, signature: Uint8Array, message: Uint8Array) =>
  crypto.subtle.verify(SIGN, key, signature as BufferSource, message as BufferSource);

describe("the key WebCrypto makes", () => {
  it("has a private half that cannot be exported in any format", async () => {
    const pair = await generatePair();

    expect(pair.privateKey.extractable).toBe(false);
    for (const format of ["pkcs8", "jwk"] as const) {
      await expect(crypto.subtle.exportKey(format, pair.privateKey)).rejects.toThrow();
    }
  });

  it("has a public half that is always exportable, as the 65-byte point the gateway takes", async () => {
    const key = await loadOrCreateDeviceKey(memoryKeyStore());

    expect(key.publicKey).toHaveLength(65);
    expect(key.publicKey[0]).toBe(0x04);
  });

  it("signs raw r || s that verifies under its own public half", async () => {
    const store = memoryKeyStore();
    const key = await loadOrCreateDeviceKey(store);
    const stored = await store.get();
    const message = new TextEncoder().encode("bytes to sign");

    const signature = await key.sign(message);

    expect(signature).toHaveLength(64);
    expect(await verifies(stored!.pair.publicKey, signature, message)).toBe(true);
  });
});

describe("IndexedDB", () => {
  beforeEach(async () => {
    await deleteDeviceKey(indexedDbKeyStore);
  });

  it("stores the CryptoKey itself, and what comes back is still non-extractable and still signs", async () => {
    const made = await loadOrCreateDeviceKey(indexedDbKeyStore);

    // A new "page load": the key comes out of storage, not out of memory.
    const record = await indexedDbKeyStore.get();
    expect(record?.pair.privateKey.extractable).toBe(false);
    await expect(crypto.subtle.exportKey("pkcs8", record!.pair.privateKey)).rejects.toThrow();

    const loaded = await loadDeviceKey(indexedDbKeyStore);
    expect(Array.from(loaded!.publicKey)).toEqual(Array.from(made.publicKey));
    const message = new TextEncoder().encode("after a reload");
    const signature = await loaded!.sign(message);
    expect(await verifies(record!.pair.publicKey, signature, message)).toBe(true);
  });

  it("holds no key material as text anywhere it can write", async () => {
    await loadOrCreateDeviceKey(indexedDbKeyStore);

    // The record is the pair and two numbers; there is no string, no JWK and no
    // PEM in it to read back.
    const record = (await indexedDbKeyStore.get()) as unknown as Record<string, unknown>;
    expect(Object.keys(record).sort()).toEqual(["createdAt", "pair"]);
    expect(JSON.stringify(record)).not.toMatch(/BEGIN|"d"\s*:/);
  });

  it("is wiped by unpairing: a browser with no key has no key", async () => {
    await loadOrCreateDeviceKey(indexedDbKeyStore);
    expect(await loadDeviceKey(indexedDbKeyStore)).toBeDefined();

    await deleteDeviceKey(indexedDbKeyStore);

    expect(await indexedDbKeyStore.get()).toBeUndefined();
    expect(await loadDeviceKey(indexedDbKeyStore)).toBeUndefined();
  });

  it("makes a different key after a wipe: re-pairing never reuses the old one", async () => {
    const first = await loadOrCreateDeviceKey(indexedDbKeyStore);
    await deleteDeviceKey(indexedDbKeyStore);
    const second = await loadOrCreateDeviceKey(indexedDbKeyStore);

    expect(Array.from(second.publicKey)).not.toEqual(Array.from(first.publicKey));
  });

  it("remembers what the gateway told it about the key, and nothing before that", async () => {
    await loadOrCreateDeviceKey(indexedDbKeyStore);
    expect((await loadDeviceKey(indexedDbKeyStore))?.paired).toBeUndefined();

    await markPaired(
      {
        keyId: "ck1",
        userId: "u1",
        pairedAt: "2026-10-27T00:00:00.000Z",
        expiresAt: "2026-11-26T00:00:00.000Z",
      },
      indexedDbKeyStore,
    );

    expect((await loadDeviceKey(indexedDbKeyStore))?.paired?.keyId).toBe("ck1");
  });
});
