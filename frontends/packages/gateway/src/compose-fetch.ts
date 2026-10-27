// The browser half of a composer call: seal the proof (`./compose-proof.ts`) and
// post it, with the message, to the console's composer route. See that file for
// why the composer needs a proof at all.

import { COMPOSE_PROOF_PATH, COMPOSE_PROOF_QUERY } from "./compose-proof";
import { NoDeviceKeyError, type SealingRuntime } from "./sealing-fetch";

/** A sealed read, as the composer route receives it. */
export interface SealedProof {
  /** The sealed request's headers, `[name, value]` pairs. */
  headers: [string, string][];
  /** The COSE message, base64. */
  body: string;
}

export interface ComposeFetchOptions {
  /** The sealing runtime, as for `createSealingFetch`. */
  runtime: () => Promise<SealingRuntime>;
  /** Where the console mounts its composer, e.g. `/api/compose`. */
  endpoint: string;
  /** Defaults to the global `fetch`. */
  send?: typeof fetch;
  /** Called for each 401 this produces or receives. */
  onUnauthenticated?: () => void;
}

function base64(bytes: Uint8Array): string {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary);
}

function refusal(status: number, code: string, message: string): Response {
  return new Response(JSON.stringify({ code, message }), {
    status,
    headers: { "content-type": "application/json" },
  });
}

/** `(operation, input) => Response` for `preview` and `send`. The answer is the
 *  composer route's own JSON: the gateway's result, or its error in the shape
 *  every other call reads (`{ code, message, details }`). */
export function createComposeFetch(
  options: ComposeFetchOptions,
): (operation: "preview" | "send", input: unknown) => Promise<Response> {
  const { runtime, endpoint, onUnauthenticated } = options;
  const send = options.send ?? ((input, init) => globalThis.fetch(input, init));

  return async (operation, input) => {
    let proof: SealedProof;
    try {
      const sealing = await runtime();
      const { route, params } = sealing.routeOf("GET", COMPOSE_PROOF_PATH);
      const sealed = await sealing.sealer.sealCall(
        "GET",
        route,
        params,
        undefined,
        COMPOSE_PROOF_QUERY,
        undefined,
        undefined,
      );
      proof = { headers: sealed.headers, body: base64(sealed.body) };
    } catch (error) {
      onUnauthenticated?.();
      return refusal(
        401,
        "unauthenticated",
        error instanceof NoDeviceKeyError
          ? "This console is not paired with a vaam admin phone."
          : "This browser's key could not sign the request. Pair this console again.",
      );
    }
    let answer: Response;
    try {
      answer = await send(`${endpoint}/${operation}`, {
        method: "POST",
        credentials: "omit",
        redirect: "error",
        cache: "no-store",
        referrerPolicy: "no-referrer",
        headers: { "content-type": "application/json", accept: "application/json" },
        body: JSON.stringify({ proof, input }),
      });
    } catch {
      return refusal(503, "unavailable", "The console could not be reached.");
    }
    if (answer.status === 401) onUnauthenticated?.();
    return answer;
  };
}
