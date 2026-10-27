/**
 * The caller's signing identity and the keys it trusts, resolved once.
 *
 * vsms has no tokens. Every request is a COSE_Sign1 message signed with an
 * **Ed25519** key whose public half an operator registered for this app
 * (`sms-gateway register-client`), and every answer is one signed by the
 * gateway's response key, which this client pins (`sms-gateway server-keys`
 * prints the pair). There is no `client_credentials` grant, no assertion, no
 * access-token cache and no re-authentication retry: `docs/integrating.md`
 * has the flow and `backends/crates/sms-api/src/envelope.md` what vsms adds to
 * cratestack's signed transport.
 */
import { createPrivateKey, createPublicKey, type KeyObject, sign } from "node:crypto";
import { readFileSync } from "node:fs";
import { SdkError } from "./error.js";
import { isWasmFailure, loadWasm, type WasmSealer } from "./wasm.js";

/** A key given as bytes or as the unpadded base64url text a key file holds. */
export type KeyBytes = string | Uint8Array;

/**
 * A signer whose key this process cannot read: a KMS, an HSM, a Node
 * `KeyObject` held elsewhere. `sign` receives the exact bytes to sign and must
 * return the 64-byte pure Ed25519 signature over them: `crypto.sign(null, tbs,
 * key)`, never a pre-hash. A signature that does not verify under `publicKey`
 * is refused before it is sent, rather than failing at the gateway as the
 * coarse `401`.
 */
export interface ExternalSigner {
  /** The registered public key: 32 bytes, or unpadded base64url. */
  publicKey: KeyBytes;
  sign(tbs: Uint8Array): Uint8Array | Promise<Uint8Array>;
}

/**
 * The gateway's pinned response keys, exactly as `sms-gateway server-keys`
 * prints them. Both are required and differ: with one pinned, a rotation is a
 * flag day, and with the same key twice it would be unobservable. Public
 * values, so plain configuration and not a secret.
 */
export interface ServerKeysOption {
  /** The key the gateway seals answers with now: 32 bytes, or unpadded base64url. */
  current: KeyBytes;
  /** The key the next rotation promotes. */
  next: KeyBytes;
}

/**
 * Parses the output of `sms-gateway server-keys` (`current: …` / `next: …`).
 *
 * @example
 * parseServerKeysReport("current: AAAA…\nnext: BBBB…\n")
 */
export function parseServerKeysReport(report: string): { current: string; next: string } {
  const field = (label: string): string => {
    for (const line of report.split(/\r?\n/)) {
      const trimmed = line.trim();
      if (trimmed.startsWith(`${label}:`)) return trimmed.slice(label.length + 1).trim();
    }
    throw new SdkError(`no \`${label}:\` line in the server keys`, { kind: "config" });
  };
  return { current: field("current"), next: field("next") };
}

/**
 * Everything that decides how calls are signed. Exactly one of
 * {@link privateKey}, {@link keyPath} and {@link signer}.
 */
export interface IdentityOptions {
  /**
   * The app's Ed25519 private key: PKCS#8 PEM text (what `openssl genpkey
   * -algorithm ed25519` writes) or a `crypto.KeyObject` (the key then stays in
   * Node and signs through `crypto.sign`), or the 32-byte RFC 8032 seed as bytes
   * or unpadded base64url.
   */
  privateKey?: string | KeyObject | Uint8Array | undefined;
  /** A file holding the PKCS#8 PEM, read once at construction. */
  keyPath?: string | undefined;
  /** A signer for a key this process cannot read. */
  signer?: ExternalSigner | undefined;
  /** The gateway's pinned response keys. */
  serverKeys: ServerKeysOption;
}

function config(message: string, cause?: unknown): SdkError {
  return new SdkError(message, { kind: "config", cause });
}

/**
 * 32 bytes as unpadded base64url. Anything else is a `config` error naming the
 * field and never quoting the value: it may be a private seed.
 */
export function toKeyBase64url(value: KeyBytes | undefined, field: string): string {
  if (value === undefined || value === null) throw config(`${field} is required`);
  if (typeof value === "string") {
    const text = value.trim();
    const bytes = Buffer.from(text, "base64url");
    if (bytes.length !== 32 || bytes.toString("base64url") !== text) {
      // The second test rejects padding and the other base64 alphabet.
      throw config(`${field} must be 32 bytes, as unpadded base64url text`);
    }
    return text;
  }
  if (value.length !== 32) throw config(`${field} must be exactly 32 bytes`);
  return Buffer.from(value).toString("base64url");
}

/** A signer over a key held in this process, with its public half already spelled. */
interface LocalSigner {
  publicKey: string;
  sign(tbs: Uint8Array): Uint8Array;
}

function signerFromKeyObject(key: KeyObject): LocalSigner {
  if (key.type !== "private" || key.asymmetricKeyType !== "ed25519") {
    throw config(
      `privateKey must be an Ed25519 private key (got ${key.asymmetricKeyType ?? key.type}); ` +
        "an app client signs with Ed25519 (a P-256 key is a console key, which a browser holds)",
    );
  }
  const jwk = createPublicKey(key).export({ format: "jwk" });
  if (typeof jwk.x !== "string") throw config("privateKey has no Ed25519 public half");
  return { publicKey: jwk.x, sign: (tbs) => sign(null, tbs, key) };
}

function signerFromPem(pem: string): LocalSigner {
  let key: KeyObject;
  try {
    key = createPrivateKey(pem);
  } catch (cause) {
    // OpenSSL's reason is the `cause`; the message never quotes the key.
    throw config(
      "privateKey could not be read: expected PKCS#8 Ed25519 PEM text, a crypto.KeyObject, or a 32-byte Ed25519 seed",
      cause,
    );
  }
  return signerFromKeyObject(key);
}

/**
 * Validates {@link IdentityOptions} and builds the sealer. Every failure it can
 * detect is detected here, at construction, rather than on the first request.
 */
export function resolveSealer(options: IdentityOptions): WasmSealer {
  const given = [options.privateKey, options.keyPath, options.signer].filter(
    (value) => value !== undefined && value !== null,
  ).length;
  if (given !== 1) {
    throw config(
      given === 0
        ? "a signing key is required: give privateKey, keyPath or signer"
        : "give exactly one of privateKey, keyPath and signer",
    );
  }
  if (options.serverKeys === undefined || options.serverKeys === null) {
    throw config("serverKeys is required: the client refuses to trust an answer it has no key for");
  }
  const current = toKeyBase64url(options.serverKeys.current, "serverKeys.current");
  const next = toKeyBase64url(options.serverKeys.next, "serverKeys.next");

  const wasm = loadWasm();
  try {
    if (options.signer !== undefined && options.signer !== null) {
      const external = options.signer;
      return wasm.Sealer.fromCallback(
        toKeyBase64url(external.publicKey, "signer.publicKey"),
        (tbs) => external.sign(tbs),
        current,
        next,
      );
    }
    let privateKey = options.privateKey;
    if (options.keyPath !== undefined) {
      try {
        privateKey = readFileSync(options.keyPath, "utf8");
      } catch (cause) {
        throw config(`the key file ${options.keyPath} could not be read`, cause);
      }
    }
    if (privateKey instanceof Uint8Array) {
      return wasm.Sealer.fromSeed(toKeyBase64url(privateKey, "privateKey"), current, next);
    }
    if (typeof privateKey === "string" && !privateKey.includes("-----BEGIN")) {
      return wasm.Sealer.fromSeed(toKeyBase64url(privateKey, "privateKey"), current, next);
    }
    if (privateKey === undefined || privateKey === null) {
      throw config("a signing key is required: give privateKey, keyPath or signer");
    }
    const signer =
      typeof privateKey === "string" ? signerFromPem(privateKey) : signerFromKeyObject(privateKey);
    return wasm.Sealer.fromCallback(signer.publicKey, (tbs) => signer.sign(tbs), current, next);
  } catch (err) {
    if (err instanceof SdkError) throw err;
    // The wasm module rejects with a plain `{ kind, message }`; its messages
    // describe a key's shape, never its bytes.
    throw config(isWasmFailure(err) ? err.message : "the signing identity could not be built", err);
  }
}
