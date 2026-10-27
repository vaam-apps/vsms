// The RFC 9679 COSE key thumbprint of a P-256 key, the name the gateway gives a
// `ConsoleKey` (`thumbprint`, lowercase hex, `@unique`) and the console shows so
// an operator can tell two paired browsers apart.
//
// It is NOT a hash of the raw point. RFC 9679 hashes the key's canonical CBOR
// map, only its required parameters, in deterministic order:
//
//   {1: 2, -1: 1, -2: x, -3: y}      kty EC2, crv P-256, x, y
//   a4 01 02 20 01 21 58 20 <x> 22 58 20 <y>
//
// and the thumbprint is SHA-256 of those bytes. `cratestack::cose` computes the
// same thing on the gateway (`backends/crates/sms-api/src/console_keys.rs`
// rebuilds it by hand in a unit test), and the real-gateway run in
// docs/design/console-pairing.md compares the two.

/** The thumbprint of an uncompressed SEC1 P-256 point (65 bytes). */
export async function coseThumbprint(publicKey: Uint8Array): Promise<string> {
  if (publicKey.length !== 65 || publicKey[0] !== 0x04) {
    throw new Error("a thumbprint is taken of an uncompressed P-256 point");
  }
  const x = publicKey.slice(1, 33);
  const y = publicKey.slice(33, 65);
  const map = new Uint8Array([
    0xa4,
    0x01,
    0x02,
    0x20,
    0x01,
    0x21,
    0x58,
    0x20,
    ...x,
    0x22,
    0x58,
    0x20,
    ...y,
  ]);
  const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", map));
  return Array.from(digest, (byte) => byte.toString(16).padStart(2, "0")).join("");
}
