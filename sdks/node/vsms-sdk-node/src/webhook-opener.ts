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
 *   audience: "vaam",                // the name the endpoint was created with
 *   route: "/webhooks/vsms",         // the whole path of the endpoint's URL
 *   appId: "capp00000000000000000",  // the vsms app the endpoint belongs to
 *   endpointId: "cend00000000000000000", // the endpoint itself: the console shows it when it is created
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
 * refused; then, inside the verified envelope, it compares `endpointId` and
 * `appId` (both required) with the ones you gave. The last step is not
 * decoration: one gateway key signs every app's deliveries, so the signature
 * and the audience only say "vsms sent this to the name `vaam`", and a delivery
 * made for someone else's endpoint that names your audience verifies in every
 * other respect. **`endpointId` is the binding**: a receiver serves exactly one
 * endpoint, and it is the one signed value no one but the gateway chooses. An
 * endpoint can be registered under your app id by anyone who may create
 * endpoints, so `appId` alone is not enough; it additionally catches an
 * endpoint registered under another app. The unique audience is defence in
 * depth, not the binding.
 *
 * **The nonce memory is per process, and unlike the Rust SDK's
 * `WebhookOpener::with_nonce_store` it cannot be replaced**: the check runs in
 * WebAssembly, which cannot await a JavaScript store. A body replayed within the
 * 300 s skew to a *different replica* (or to this one after a restart) opens a
 * second time. A receiver with several replicas, or one that must survive a
 * restart, dedupes on `event.id` as well, which delivery being at-least-once
 * asks of every receiver anyway.
 *
 * `open` takes the raw request bytes only. A body a framework has already
 * decoded to a string is not those bytes, and re-encoding it can change them
 * (a lone surrogate, a BOM, a different charset), which a signature cannot
 * survive: read the body raw (`express.raw`, `request.arrayBuffer()`).
 */
import { SdkError } from "./error.js";
import { type ServerKeysOption, toKeyBase64url } from "./identity.js";
import { isWasmFailure, loadWasm, type WasmWebhookOpener } from "./wasm.js";

export interface WebhookOpenerOptions {
  /** The gateway's pinned keys, as for a `VsmsClient`. */
  serverKeys: ServerKeysOption;
  /** The name this receiver chose when its endpoint was created: what the delivery is addressed to. */
  audience: string;
  /** The whole path of the endpoint's URL, such as `/webhooks/vsms`. */
  route: string;
  /**
   * The id of the vsms app the endpoint belongs to (`WebhookEndpoint.appId`).
   * Required. A delivery made for another app is refused as `unverified`.
   */
  appId: string;
  /**
   * The id of the endpoint this receiver serves (`WebhookEndpoint.id`; the
   * console shows it when the endpoint is created). Required, and the binding:
   * a delivery made through any other endpoint, of your app or not, is refused
   * as `unverified`.
   */
  endpointId: string;
  /** A fixed clock in whole Unix seconds. For tests and for replaying a recorded delivery. */
  nowSeconds?: number | undefined;
}

/** An opened webhook event (`docs/architecture.md` section 8.4). */
export interface WebhookEvent {
  /** `WebhookAttempt.id`: the dedupe key. A retry and an operator's replay carry the same one. */
  id: string;
  /** The source event's id: what the `X-Sms-Event-Id` header carried before webhooks were sealed. */
  eventId: string;
  /** The app the endpoint belongs to; the opener has already compared it with `appId`. */
  appId: string;
  /** The endpoint the delivery was made through. */
  endpointId: string;
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
    if (typeof options.appId !== "string" || options.appId === "") {
      throw new SdkError(
        "appId is required: the audience alone does not say whose delivery it is",
        { kind: "config" },
      );
    }
    if (typeof options.endpointId !== "string" || options.endpointId === "") {
      throw new SdkError(
        "endpointId is required: the audience and the app id do not say which endpoint of the app a delivery was made through",
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
        options.appId,
        options.endpointId,
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
   * `body` is the raw request body, as bytes: a `string` is refused (`config`),
   * because a framework that decoded the body may not give the same bytes back.
   *
   * Rejects with an {@link SdkError} of kind `unverified` (the body does not
   * verify under a pinned key for this audience and route, or verifies and was
   * made for another app or endpoint: answer `401`; it is one answer for a
   * forgery, a replay, a stale timestamp and every other reason, on purpose),
   * `unreadable` (it verified and is not UTF-8), `decode` (it verified and is
   * not the envelope vsms documents) or `backend`.
   */
  async open(body: Uint8Array): Promise<WebhookEvent> {
    if (!(body instanceof Uint8Array)) {
      throw new SdkError(
        "the delivery must be the raw request body as bytes (a Buffer or Uint8Array), not decoded text",
        { kind: "config" },
      );
    }
    let text: string;
    try {
      text = await this.#opener.open(body);
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
    case "malformed":
      return new SdkError(err.message, { kind: "decode" });
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
    appId: field("appId"),
    endpointId: field("endpointId"),
    type: field("type"),
    occurredAt: field("occurredAt"),
    data: record.data,
  };
}
