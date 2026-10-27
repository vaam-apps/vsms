// The console's signing key: a P-256 pair WebCrypto made, whose private half the
// page can ask to sign with but can never read (`extractable: false`), kept in
// IndexedDB as the `CryptoKey` object itself. Nothing here holds key material in
// a string, a cookie or `localStorage`, so there is nothing to steal by reading
// storage. What a script on the page could still do is ask the key to sign while
// the page is open; the answer to that is the gateway's, not this file's: the key
// belongs to an `admin` (never an `owner`), lasts 30 days, and is revoked from
// the console it lives in or from any other (`docs/design/console-pairing.md`,
// "What a stolen browser profile gets").
//
// The record also carries what the browser has learned about its own key (when
// the gateway first accepted it, and when it expires), because the gateway
// cannot tell a key it never knew from a key it has finished with: both are the
// same unsigned 401. The browser can, and it is the only place that can say
// "this console WAS paired" and so ask to pair again rather than wait.

const ECDSA = { name: "ECDSA", namedCurve: "P-256" } as const;
const SIGN = { name: "ECDSA", hash: "SHA-256" } as const;

/** What the browser knows about its key's life, written once the gateway has
 *  accepted it. */
export interface PairedMeta {
  /** The gateway's id for the key, what `revokeConsoleKey` takes. */
  keyId: string;
  /** The gateway's id for the person the key speaks as. */
  userId: string;
  /** ISO 8601: when the gateway stops accepting the key (30 days after pairing). */
  expiresAt: string;
  /** ISO 8601. */
  pairedAt: string;
}

/** The IndexedDB record: the pair, and what is known about it. */
export interface StoredKey {
  pair: CryptoKeyPair;
  /** Milliseconds since the epoch: when this browser made the key. */
  createdAt: number;
  /** Present once the gateway has accepted the key. */
  paired?: PairedMeta | undefined;
}

/** Where a key lives between page loads: IndexedDB in a browser, a Map in a
 *  test. A value is structured-cloned, which is how a non-extractable key is
 *  stored at all. */
export interface KeyStore {
  get(): Promise<StoredKey | undefined>;
  put(record: StoredKey): Promise<void>;
  clear(): Promise<void>;
}

/** The browser's key: its public half in the forms the gateway and the QR use,
 *  and a function that signs with the private half. */
export interface DeviceKey {
  /** Uncompressed SEC1 point, 65 bytes (`0x04 ‖ x ‖ y`). */
  readonly publicKey: Uint8Array;
  /** ECDSA P-256 over SHA-256 of `bytes`: 64 bytes, `r ‖ s`. */
  sign(bytes: Uint8Array): Promise<Uint8Array>;
  readonly createdAt: number;
  readonly paired: PairedMeta | undefined;
}

const DATABASE = "vsms-console";
const STORE = "device-key";
const RECORD = "browser";

function request<T>(req: IDBRequest<T>): Promise<T> {
  return new Promise((resolve, reject) => {
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error ?? new Error("IndexedDB request failed"));
  });
}

function open(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const opening = indexedDB.open(DATABASE, 1);
    opening.onupgradeneeded = () => opening.result.createObjectStore(STORE);
    opening.onsuccess = () => resolve(opening.result);
    opening.onerror = () => reject(opening.error ?? new Error("IndexedDB could not be opened"));
  });
}

/** The key store a browser uses. */
export const indexedDbKeyStore: KeyStore = {
  async get() {
    const db = await open();
    try {
      const found = await request(db.transaction(STORE).objectStore(STORE).get(RECORD));
      return found as StoredKey | undefined;
    } finally {
      db.close();
    }
  },
  async put(record) {
    const db = await open();
    try {
      await request(db.transaction(STORE, "readwrite").objectStore(STORE).put(record, RECORD));
    } finally {
      db.close();
    }
  },
  async clear() {
    const db = await open();
    try {
      await request(db.transaction(STORE, "readwrite").objectStore(STORE).delete(RECORD));
    } finally {
      db.close();
    }
  },
};

/** A key store held in memory: for tests, and for nothing else. */
export function memoryKeyStore(): KeyStore {
  let held: StoredKey | undefined;
  return {
    get: async () => held,
    put: async (record) => {
      held = record;
    },
    clear: async () => {
      held = undefined;
    },
  };
}

/** A fresh non-extractable pair. `extractable: false` makes the private half
 *  unexportable in every format; the public half is always exportable, which is
 *  all the QR and the gateway need. */
export async function generatePair(): Promise<CryptoKeyPair> {
  return (await crypto.subtle.generateKey(ECDSA, false, ["sign", "verify"])) as CryptoKeyPair;
}

async function toDeviceKey(record: StoredKey): Promise<DeviceKey> {
  const publicKey = new Uint8Array(await crypto.subtle.exportKey("raw", record.pair.publicKey));
  if (publicKey.length !== 65 || publicKey[0] !== 0x04) {
    throw new Error("the browser's public key is not an uncompressed P-256 point");
  }
  return {
    publicKey,
    createdAt: record.createdAt,
    paired: record.paired,
    async sign(bytes) {
      // WebCrypto answers the raw `r ‖ s` the envelope's signer takes as is.
      return new Uint8Array(
        await crypto.subtle.sign(SIGN, record.pair.privateKey, bytes as BufferSource),
      );
    },
  };
}

/** The stored key, if this browser has been given one. Never makes one: a page
 *  without a key sends the person to /pair instead. */
export async function loadDeviceKey(
  store: KeyStore = indexedDbKeyStore,
): Promise<DeviceKey | undefined> {
  const record = await store.get();
  return record ? toDeviceKey(record) : undefined;
}

/** The stored key, or a fresh non-extractable one made and stored now. Only
 *  the pairing page calls this. */
export async function loadOrCreateDeviceKey(
  store: KeyStore = indexedDbKeyStore,
  now: () => number = Date.now,
): Promise<DeviceKey> {
  const existing = await store.get();
  if (existing) return toDeviceKey(existing);
  const record: StoredKey = { pair: await generatePair(), createdAt: now() };
  await store.put(record);
  return toDeviceKey(record);
}

/** Records that the gateway accepted the key, and when it stops doing so. */
export async function markPaired(
  meta: PairedMeta,
  store: KeyStore = indexedDbKeyStore,
): Promise<void> {
  const record = await store.get();
  if (record) await store.put({ ...record, paired: meta });
}

/** Forgets the key: unpairing this browser. The gateway's row for it stays until
 *  it expires or is revoked (`revokeConsoleKey`), which is why revoking comes
 *  first when the key should stop working everywhere. */
export async function deleteDeviceKey(store: KeyStore = indexedDbKeyStore): Promise<void> {
  await store.clear();
}
