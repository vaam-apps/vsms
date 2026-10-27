import "server-only";

// The composer's route: `previewMessage` and `sendMessage` through the console's
// own service key.
//
// Both procedures are for an app principal only
// (`backends/crates/sms-api/src/procedures.rs::caller_client_id`: "deriving an
// App for a human caller has no design yet"), so a paired person's browser key
// cannot call them, and nothing else in the console needs the service key. The
// key is an Ed25519 `AppClient` key the deployment registered with
// `sms-gateway register-client --client-name console-composer`
// (`docs/runbooks/deployment.adoc`, "The composer's key"); this file signs with
// it through the Node SDK, which seals every call and believes only an answer
// that verifies under the gateway's pinned keys.
//
// **A standing key must not be spent on whoever reaches the route.** There is no
// session here to ask, so the caller proves itself with the only thing it has: a
// request its own key signed. The browser seals one cheap read
// (`./compose-proof.ts`) and posts it with the message; this handler forwards
// those bytes to the gateway and goes on only if the gateway answers 200, which
// it does for a paired, unexpired, unrevoked console key and for nothing else.
// The proof is single-use (the gateway refuses a nonce it has seen) and at most
// five minutes old. It is not bound to the message, which is a deliberate
// limit: it proves a paired operator is at the keyboard, not what they typed,
// and anyone who can make the browser sign can already make it sign anything.
//
// R4 holds: the gateway never calls this. Delete the console and the service
// key does nothing; a deployment that wants to send without a console uses the
// SDK with the same `register-client` flow.

import type { SealedProof } from "../compose-fetch";
import { COMPOSE_PROOF_TARGET } from "../compose-proof";
import { MAX_REQUEST_BYTES, pick, readBounded, type Upstream } from "./pass-through";

/** The part of `@vaam-apps/vsms-node`'s `VsmsClient` the composer uses. */
export interface ComposerClient {
  previewMessage(input: never): Promise<unknown>;
  sendMessage(input: never): Promise<{ result: unknown }>;
}

/** The part of `@vaam-apps/vsms-node`'s `SdkError` the composer reads. */
interface SdkFailure {
  kind: string;
  message: string;
  httpStatus?: number | undefined;
  gatewayCode?: string | undefined;
  fieldErrors?: Record<string, string[]> | undefined;
}

function isSdkFailure(error: unknown): error is SdkFailure {
  return (
    error instanceof Error &&
    error.name === "SdkError" &&
    typeof (error as { kind?: unknown }).kind === "string"
  );
}

/** The headers of a proof that go to the gateway: the ones a seal depends on. */
const PROOF_HEADERS: readonly string[] = [
  "content-type",
  "accept",
  "cratestack-contract",
  "cratestack-payload-type",
  "cratestack-payload-accept",
];

function json(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json", "cache-control": "no-store" },
  });
}

function parseProof(value: unknown): { headers: [string, string][]; body: Uint8Array } | undefined {
  if (typeof value !== "object" || value === null) return undefined;
  const { headers, body } = value as Partial<SealedProof>;
  if (!Array.isArray(headers) || typeof body !== "string") return undefined;
  const pairs: [string, string][] = [];
  for (const pair of headers) {
    if (
      !Array.isArray(pair) ||
      pair.length !== 2 ||
      typeof pair[0] !== "string" ||
      typeof pair[1] !== "string"
    ) {
      return undefined;
    }
    pairs.push([pair[0], pair[1]]);
  }
  const bytes = Buffer.from(body, "base64");
  if (bytes.byteLength === 0 || bytes.byteLength > MAX_REQUEST_BYTES) return undefined;
  return { headers: pairs, body: new Uint8Array(bytes) };
}

export interface ComposerOptions {
  /** Forwards the proof to the gateway. */
  upstream: Upstream;
  /** The service key's client, built when first needed (a build has no key). */
  client: () => ComposerClient;
}

/** The handler of `POST <mount>/<preview|send>`. */
export function createComposer(options: ComposerOptions): (request: Request) => Promise<Response> {
  return async (request) => {
    if (request.method !== "POST") return json(405, { code: "refused", message: "POST only." });
    const operation = new URL(request.url).pathname.split("/").pop();
    if (operation !== "preview" && operation !== "send") {
      return json(404, { code: "refused", message: "Not a composer route." });
    }
    const raw = await readBounded(request, 2 * MAX_REQUEST_BYTES);
    if (raw === undefined) return json(413, { code: "refused", message: "Too large." });
    let parsed: { proof?: unknown; input?: unknown };
    try {
      parsed = JSON.parse(new TextDecoder().decode(raw)) as typeof parsed;
    } catch {
      return json(400, { code: "invalid_argument", message: "The body is not JSON." });
    }
    const proof = parseProof(parsed.proof);
    if (proof === undefined || typeof parsed.input !== "object" || parsed.input === null) {
      return json(400, { code: "invalid_argument", message: "A proof and an input are required." });
    }

    // Who is asking? Whoever the gateway says: it answers 200 to a sealed read
    // from a paired console key, and with its unsigned 401 to anything else.
    try {
      const verdict = await options.upstream({
        method: "GET",
        target: COMPOSE_PROOF_TARGET,
        headers: pick(new Headers(proof.headers), PROOF_HEADERS),
        body: proof.body,
      });
      if (verdict.status !== 200) {
        return json(401, {
          code: "unauthenticated",
          message: "This console is not paired with a vaam admin phone.",
        });
      }
    } catch {
      return json(503, { code: "unavailable", message: "The gateway could not be reached." });
    }

    try {
      const client = options.client();
      if (operation === "preview") {
        return json(200, await client.previewMessage(parsed.input as never));
      }
      const { result } = await client.sendMessage(parsed.input as never);
      return json(200, result);
    } catch (error) {
      if (isSdkFailure(error) && error.kind === "remote" && error.httpStatus !== undefined) {
        return json(error.httpStatus, {
          code: error.gatewayCode ?? "error",
          message: error.message,
          ...(error.fieldErrors !== undefined ? { details: error.fieldErrors } : {}),
        });
      }
      if (isSdkFailure(error)) {
        // The SDK refused to believe an answer, or could not sign: the service
        // key is misconfigured or the gateway's keys moved. Say so, not why.
        console.error(
          `[@vsms/gateway] the composer's call failed (${error.kind}): ${error.message}`,
        );
        return json(502, {
          code: "composer_unavailable",
          message: "The composer's service key could not complete the call. Ask an operator.",
        });
      }
      console.error("[@vsms/gateway] the composer failed", error);
      return json(500, { code: "internal", message: "The composer failed." });
    }
  };
}
