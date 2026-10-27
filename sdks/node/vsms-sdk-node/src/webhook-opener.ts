/**
 * Opening a sealed webhook (VS-8).
 *
 * vsms delivers an event as a sealed request: a `COSE_Sign1` message signed with
 * the gateway's own envelope signing key, the one that signs its answers. There
 * is no shared secret and no signature header. A receiver pins the two public
 * keys it already pins to open vsms's answers (`sms-gateway server-keys`) and
 * opens the request body here:
 *
 * ```ts
 * const opener = new WebhookOpener({
 *   serverKeys: parseServerKeysReport(readFileSync("server-keys.txt", "utf8")),
 *   audience: "vaam",          // the name the endpoint was created with
 *   route: "/webhooks/vsms",   // the path of the endpoint's URL
 * });
 * app.post("/webhooks/vsms", express.raw({ type: "*\/*" }), async (req, res) => {
 *   try {
 *     const event = await opener.open(req.body);
 *     // ... dedupe on event.id, act on event.type and event.data ...
 *     res.sendStatus(204);
 *   } catch (err) {
 *     res.sendStatus(err instanceof SdkError && err.kind === "unverified" ? 401 : 500);
 *   }
 * });
 * ```
 *
 * The check is cratestack's envelope through the same WebAssembly module the
 * client seals with (`sdks/rust/vsms-sdk-wasm`), not a TypeScript copy. It binds
 * the audience, `POST`, the route, the delivery contract, the payload type, the
 * timestamp (300 s) and the nonce, which is remembered so a replayed body is
 * refused. **The nonce memory is per process**: a receiver with several replicas
 * should dedupe on `event.id` as well, which delivery being at-least-once asks
 * of every receiver anyway.
 */
import { SdkError } from "./error.js";
import { type ServerKeysOption, toKeyBase64url } from "./identity.js";
import { isWasmFailure, loadWasm, type WasmWebhookOpener } from "./wasm.js";

export interface WebhookOpenerOptions {
  /** The gateway's pinned keys, as for a `VsmsClient`. */
  serverKeys: ServerKeysOption;
  /** The name this receiver chose when its endpoint was created: what the delivery is addressed to. */
  audience: string;
  /** The path of the endpoint's URL, such as `/webhooks/vsms`. */
  route: string;
  /** A fixed clock in whole Unix seconds. For tests and for replaying a recorded delivery. */
  nowSeconds?: number | undefined;
}

/** An opened webhook event (`docs/architecture.md` section 8.4). */
export interface WebhookEvent {
  /** `WebhookAttempt.id`: the dedupe key. A retry and an operator's replay carry the same one. */
  id: string;
  /** The source event's id: what the `X-Sms-Event-Id` header carried before webhooks were sealed. */
  eventId: string;
  /** `message.delivered` and the rest of the catalogue. */
  type: string;
  /** When the delivery was made, RFC 3339 in UTC: an approximation of when the event happened. */
  occurredAt: string;
  /** The event's payload, exactly as vsms sent it. */
  data: unknown;
}

export class WebhookOpener {
  readonly #opener: WasmWebhookOpener;

  constructor(options: WebhookOpenerOptions) {
    if (options.serverKeys === undefined || options.serverKeys === null) {
      throw new SdkError(
        "serverKeys is required: the opener refuses to trust a delivery it has no key for",
        { kind: "config" },
      );
    }
    const current = toKeyBase64url(options.serverKeys.current, "serverKeys.current");
    const next = toKeyBase64url(options.serverKeys.next, "serverKeys.next");
    try {
      this.#opener = loadWasm().WebhookOpener.forServerKeys(
        current,
        next,
        options.audience,
        options.route,
        options.nowSeconds,
      );
    } catch (err) {
      if (err instanceof SdkError) throw err;
      throw new SdkError(
        isWasmFailure(err) ? err.message : "the webhook opener could not be built",
        {
          kind: "config",
          cause: err,
        },
      );
    }
  }

  /**
   * The event in one delivery's raw request body.
   *
   * Rejects with an {@link SdkError} of kind `unverified` (the body does not
   * verify under a pinned key for this audience and route: answer `401`; it is
   * one answer for a forgery, a replay, a stale timestamp and every other
   * reason, on purpose), `unreadable` (it verified and is not UTF-8), `decode`
   * (it verified and is not the envelope vsms documents) or `backend`.
   */
  async open(body: Uint8Array | string): Promise<WebhookEvent> {
    const bytes = typeof body === "string" ? new TextEncoder().encode(body) : body;
    let text: string;
    try {
      text = await this.#opener.open(bytes);
    } catch (err) {
      throw webhookFailure(err);
    }
    return parseEvent(text);
  }
}

function webhookFailure(err: unknown): SdkError {
  if (!isWasmFailure(err)) {
    return new SdkError("the delivery could not be checked", { kind: "backend", cause: err });
  }
  switch (err.kind) {
    case "unverified":
      return new SdkError(err.message, { kind: "unverified" });
    case "unreadable":
      return new SdkError(err.message, { kind: "unreadable" });
    default:
      return new SdkError(err.message, { kind: "backend" });
  }
}

function parseEvent(text: string): WebhookEvent {
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch (cause) {
    throw new SdkError("the delivery verified but is not JSON", { kind: "decode", cause });
  }
  const record = parsed as Record<string, unknown> | null;
  const field = (name: string): string => {
    const value = record?.[name];
    if (typeof value !== "string") {
      throw new SdkError(`the delivery verified but has no string \`${name}\``, { kind: "decode" });
    }
    return value;
  };
  if (record === null || typeof record !== "object" || !("data" in record)) {
    throw new SdkError("the delivery verified but is not a webhook envelope", { kind: "decode" });
  }
  return {
    id: field("id"),
    eventId: field("eventId"),
    type: field("type"),
    occurredAt: field("occurredAt"),
    data: record.data,
  };
}
