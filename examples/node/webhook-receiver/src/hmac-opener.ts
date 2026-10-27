import { SdkError } from "@vaam-apps/vsms-node";
import { verifySignature } from "./signature.ts";
import type { Delivery, ReceivedEvent } from "./types.ts";

/**
 * Opens an `hmac` delivery (`WebhookEndpoint.deliveryMode = "hmac"`): the
 * original scheme, `X-Sms-Signature` over the exact bytes received, keyed by
 * the endpoint's `secret` and, during a rotation's overlap window, its
 * `prevSecret` too (`signature.ts`).
 *
 * It is the `hmac` counterpart of the SDK's `WebhookOpener` and fails the same
 * way, so `server.ts` treats both alike: an `SdkError` of kind `unverified` for
 * a signature that does not verify (a 401, which vsms retries), `decode` for a
 * verified body that is not the envelope (a 400).
 *
 * What it does not do, and the `sealed` mode does, on purpose and because this
 * scheme cannot: it does not bind the delivery to an app, an endpoint or an
 * audience (the secret is the endpoint's own and nobody else holds it), and it
 * checks no freshness window on `X-Sms-Timestamp` (`signature.ts`'s module doc
 * has why; dedupe on the event id is what stops a replay being acted on twice).
 *
 * The event's `id` is the signed `X-Sms-Event-Id`, the dedupe key this mode has
 * always documented (`WebhookAttempt.sourceEventId`); the body's own `id`
 * (`WebhookAttempt.id`) is as stable across retries, but the header is what the
 * signature covers by name.
 */
export class HmacOpener {
  readonly #secrets: readonly string[];

  /** `secret` is the endpoint's current secret; `prevSecret` its previous one while a rotation is open. */
  constructor(secret: string, prevSecret?: string) {
    this.#secrets = [secret, prevSecret].filter((s): s is string => s !== undefined && s !== "");
    if (this.#secrets.length === 0) {
      throw new SdkError("an hmac receiver needs the endpoint's secret", { kind: "config" });
    }
  }

  async open({ body, headers }: Delivery): Promise<ReceivedEvent> {
    const header = (name: string): string | undefined => {
      const value = headers[name];
      return Array.isArray(value) ? value.join(",") : value;
    };
    const eventId = header("x-sms-event-id");
    const verdict = verifySignature({
      rawBody: Buffer.from(body),
      timestamp: header("x-sms-timestamp"),
      eventId,
      signatureHeader: header("x-sms-signature"),
      secrets: this.#secrets,
    });
    if (!verdict.ok || eventId === undefined) {
      throw new SdkError(verdict.ok ? "missing X-Sms-Event-Id" : verdict.reason, {
        kind: "unverified",
      });
    }
    let envelope: unknown;
    try {
      envelope = JSON.parse(Buffer.from(body).toString("utf8"));
    } catch (cause) {
      throw new SdkError("the verified body is not JSON", { kind: "decode", cause });
    }
    const { type, occurredAt, data } = (envelope ?? {}) as Record<string, unknown>;
    if (typeof type !== "string" || typeof occurredAt !== "string") {
      throw new SdkError("the verified body is not the delivery envelope", { kind: "decode" });
    }
    return { id: eventId, type, occurredAt, data };
  }
}
