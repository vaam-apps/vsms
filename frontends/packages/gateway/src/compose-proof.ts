// The proof a browser attaches to a composer call.
//
// The composer sends through the console's own service key, held by the
// console's server (`./bff/composer.ts`), because `previewMessage` and
// `sendMessage` are procedures only an app may call. That key is a standing
// authority, so the server must not spend it for whoever happens to reach the
// route, and it must not take "the gateway accepted a signature" for "a person
// who may send signed": the gateway answers 200 to every signer its envelope
// accepts, an app client's key as much as a paired person's.
//
// So the browser seals a call to `authorizeComposition`, a procedure only a
// person holding `sms:send` can be answered, naming the operation and the
// SHA-256 of the exact text of the input it is about to post. The server
// forwards those bytes, opens the sealed answer under the gateway's pinned keys
// and goes on only if the answer names a person, that operation and that hash.
// A proof is therefore good for one message, once (the gateway refuses a nonce
// it has seen) and for five minutes at most; and it proves a paired operator
// who may send is at the keyboard, not that what they typed was wise.
//
// Both halves, the browser that seals it and the server that checks what it was
// sent, import these constants so the route is written once.

/** The route of the proof call. */
export const COMPOSE_PROOF_PATH = "/$procs/authorizeComposition";
/** The method of the proof call. */
export const COMPOSE_PROOF_METHOD = "POST";
/** The operations of the composer. */
export const COMPOSE_OPERATIONS = ["send", "preview"] as const;
export type ComposeOperation = (typeof COMPOSE_OPERATIONS)[number];

/** What the gateway's answer to the proof names: the person, and what they asked for. */
export interface CompositionAuthorization {
  userId: string;
  role: string;
  operation: string;
  inputSha256: string;
}

/** The JSON body of the proof call. */
export function composeProofBody(operation: ComposeOperation, inputSha256: string): string {
  return JSON.stringify({ args: { operation, inputSha256 } });
}

/** SHA-256 of `text` as UTF-8, 64 lowercase hexadecimal digits. The one place
 *  either half computes it. */
export async function sha256Hex(text: string): Promise<string> {
  const digest = await globalThis.crypto.subtle.digest("SHA-256", new TextEncoder().encode(text));
  return [...new Uint8Array(digest)].map((byte) => byte.toString(16).padStart(2, "0")).join("");
}

/** The authorization in a gateway answer's JSON text, or `undefined` for text
 *  that is not one. */
export function parseAuthorization(text: string): CompositionAuthorization | undefined {
  let value: unknown;
  try {
    value = JSON.parse(text);
  } catch {
    return undefined;
  }
  if (typeof value !== "object" || value === null) return undefined;
  const { userId, role, operation, inputSha256 } = value as Record<string, unknown>;
  if (
    typeof userId !== "string" ||
    userId === "" ||
    typeof role !== "string" ||
    typeof operation !== "string" ||
    typeof inputSha256 !== "string"
  ) {
    return undefined;
  }
  return { userId, role, operation, inputSha256 };
}
