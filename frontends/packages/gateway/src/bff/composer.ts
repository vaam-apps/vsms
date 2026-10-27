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
// **A standing key must not be spent on whoever reaches the route, and "the
// gateway answered 200" does not say who signed**: the gateway answers 200 to
// every signer its envelope accepts, an app client's key as well as a paired
// person's. So the proof is a call to `authorizeComposition`
// (`../compose-proof.ts`), a procedure the gateway answers only for a signed-in
// person holding `sms:send`, and this handler:
//
//   1. hashes the text of the input it received, and requires the operation of
//      the route;
//   2. forwards the proof's sealed bytes to the gateway;
//   3. opens the sealed answer under the gateway's two pinned keys, against
//      those very bytes (`AnswerOpener`), so that nothing but the gateway can
//      have said it, and about nothing but this request;
//   4. goes on only if the opened answer names a person, that operation and that
//      hash.
//
// Only then does the service key sign. A proof is for one message: it cannot
// send another (the hash differs), and it cannot be sent twice (the gateway
// refuses a nonce it has seen, and the answer to a second presentation is the
// unsigned 401). It is at most five minutes old. What it still does not prove
// is that what the operator typed was wise.
//
// R4 holds: the gateway never calls this. Delete the console and the service
// key does nothing; a deployment that wants to send without a console uses the
// SDK with the same `register-client` flow.

import type { SealedProof } from "../compose-fetch";
import {
  COMPOSE_PROOF_METHOD,
  COMPOSE_PROOF_PATH,
  type ComposeOperation,
  parseAuthorization,
  sha256Hex,
} from "../compose-proof";
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

/** The part of `@vaam-apps/vsms-node`'s `AnswerOpener` the composer uses: it
 *  rejects with an `SdkError` unless the answer is the gateway's, to exactly
 *  this request. */
export interface ComposerOpener {
  open(
    request: {
      method: string;
      target: string;
      body: Uint8Array;
      headers?: [string, string][];
    },
    answer: { status: number; headers: [string, string][]; body: Uint8Array },
  ): Promise<string>;
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

/** An HTTP field name (RFC 9110 token): what `new Headers` accepts. */
const HEADER_NAME = /^[!#$%&'*+.^_`|~0-9A-Za-z-]+$/;
/** A field value `new Headers` accepts and that cannot smuggle a line: tab,
 *  visible ASCII and space, and the obs-text octets — nothing above U+00FF,
 *  which `Headers` would throw on. */
const HEADER_VALUE = /^[\t\x20-\x7e\x80-\xff]*$/;

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
      typeof pair[1] !== "string" ||
      !HEADER_NAME.test(pair[0]) ||
      !HEADER_VALUE.test(pair[1])
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
  /** Opens the gateway's answer to the proof under its pinned keys. */
  opener: () => ComposerOpener;
}

/** `undefined` when the gateway has vouched for a person who may `operation`
 *  this very input, and otherwise the response that refuses. */
async function verdictOf(
  options: ComposerOptions,
  operation: ComposeOperation,
  inputSha256: string,
  proof: { headers: [string, string][]; body: Uint8Array },
): Promise<Response | undefined> {
  const forwarded = pick(new Headers(proof.headers), PROOF_HEADERS);
  let answer: Awaited<ReturnType<Upstream>>;
  try {
    answer = await options.upstream({
      method: COMPOSE_PROOF_METHOD,
      target: COMPOSE_PROOF_PATH,
      headers: forwarded,
      body: proof.body,
    });
  } catch {
    return json(503, { code: "unavailable", message: "The gateway could not be reached." });
  }

  let text: string;
  try {
    text = await options.opener().open(
      {
        method: COMPOSE_PROOF_METHOD,
        target: COMPOSE_PROOF_PATH,
        body: proof.body,
        headers: forwarded,
      },
      { status: answer.status, headers: [...answer.headers.entries()], body: answer.body },
    );
  } catch (error) {
    if (isSdkFailure(error) && error.kind === "unsigned" && error.httpStatus === 401) {
      // The gateway's coarse refusal of a signature it does not accept: this
      // browser's key is not a paired one, or the proof was spent or stale.
      return json(401, {
        code: "unauthenticated",
        message: "This console is not paired with a vaam admin phone.",
      });
    }
    console.error(
      `[@vsms/gateway] the gateway's answer to a composer proof was not believed (${
        isSdkFailure(error) ? error.kind : "unknown"
      })`,
    );
    return json(502, {
      code: "unverified",
      message: "The answer from the gateway could not be verified, so nothing was sent.",
    });
  }

  if (answer.status === 403) {
    return json(403, {
      code: "forbidden",
      message: "This person is not allowed to send messages from the console.",
    });
  }
  const authorization = answer.status === 200 ? parseAuthorization(text) : undefined;
  if (authorization === undefined) {
    return json(502, {
      code: "unverified",
      message: "The gateway did not name the person asking, so nothing was sent.",
    });
  }
  if (authorization.operation !== operation || authorization.inputSha256 !== inputSha256) {
    return json(403, {
      code: "forbidden",
      message: "This proof was not made for this message.",
    });
  }
  return undefined;
}

/** True when `text` holds no lone surrogate: UTF-8 round-trips it unchanged. */
function isWellFormed(text: string): boolean {
  return new TextDecoder().decode(new TextEncoder().encode(text)) === text;
}

/** The handler of `POST <mount>/<preview|send>`. */
export function createComposer(options: ComposerOptions): (request: Request) => Promise<Response> {
  return async (request) => {
    if (request.method !== "POST") return json(405, { code: "refused", message: "POST only." });
    const named = new URL(request.url).pathname.split("/").pop();
    if (named !== "preview" && named !== "send") {
      return json(404, { code: "refused", message: "Not a composer route." });
    }
    const operation: ComposeOperation = named;
    const raw = await readBounded(request, 2 * MAX_REQUEST_BYTES);
    if (raw === undefined) return json(413, { code: "refused", message: "Too large." });
    let parsed: { proof?: unknown; input?: unknown };
    try {
      parsed = JSON.parse(new TextDecoder().decode(raw)) as typeof parsed;
    } catch {
      return json(400, { code: "invalid_argument", message: "The body is not JSON." });
    }
    const proof = parseProof(parsed.proof);
    if (proof === undefined || typeof parsed.input !== "string" || parsed.input === "") {
      return json(400, { code: "invalid_argument", message: "A proof and an input are required." });
    }
    // The text that was hashed is the text that is sent: the proof is for these
    // bytes and no others.
    let input: unknown;
    try {
      input = JSON.parse(parsed.input);
    } catch {
      return json(400, { code: "invalid_argument", message: "The input is not JSON." });
    }
    if (typeof input !== "object" || input === null || Array.isArray(input)) {
      return json(400, { code: "invalid_argument", message: "The input is not an object." });
    }
    // A lone surrogate encodes as U+FFFD, so two texts would share one hash.
    if (!isWellFormed(parsed.input)) {
      return json(400, { code: "invalid_argument", message: "The input is not well-formed text." });
    }
    const inputSha256 = await sha256Hex(parsed.input);

    // Who is asking? Whoever the gateway says, in a sealed answer that names
    // this operation and this input.
    const refusal = await verdictOf(options, operation, inputSha256, proof);
    if (refusal !== undefined) return refusal;

    try {
      const client = options.client();
      if (operation === "preview") {
        return json(200, await client.previewMessage(input as never));
      }
      const { result } = await client.sendMessage(input as never);
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
