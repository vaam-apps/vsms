import { readFileSync } from "node:fs";
import { parseServerKeysReport, WebhookOpener } from "@vaam-apps/vsms-node";
import { HmacOpener } from "./hmac-opener.ts";
import type { EventOpener } from "./server.ts";

/** The route this receiver serves, and so the path of the `WebhookEndpoint.url` that points at it. */
export const ROUTE = "/webhooks/vsms";

/** The audience when none is configured: the name the example's endpoint is created with. */
export const DEFAULT_AUDIENCE = "webhook-receiver-example";

/**
 * How this receiver's endpoint is authenticated: the `deliveryMode` the
 * `WebhookEndpoint` was created with. A receiver serves exactly one endpoint, so
 * it has exactly one mode, and a delivery in the other is refused.
 *
 * - `sealed` (the default, and the recommended one): a sealed request signed
 *   with the gateway's own key. Nothing secret is configured.
 * - `hmac`: the original `X-Sms-Signature` HMAC under the endpoint's own secret,
 *   for a receiver that has not moved to the seal. A deployment will later be
 *   able to switch this mode off (VSA-1).
 */
export type ReceiverMode = "sealed" | "hmac";

/**
 * `WEBHOOK_RECEIVER_MODE`: `sealed` (default) or `hmac`. Anything else refuses to
 * start, so a typo cannot silently pick the other scheme.
 */
export function modeFromEnv(env: NodeJS.ProcessEnv = process.env): ReceiverMode {
  const mode = env.WEBHOOK_RECEIVER_MODE;
  if (mode === undefined || mode === "" || mode === "sealed") return "sealed";
  if (mode === "hmac") return "hmac";
  throw new Error(`WEBHOOK_RECEIVER_MODE must be "sealed" or "hmac", not ${JSON.stringify(mode)}`);
}

/** The seam over the SDK's opener, which checks the body alone. */
export function sealedOpener(opener: WebhookOpener): EventOpener {
  return { open: ({ body }) => opener.open(body) };
}

/**
 * Builds the opener from the environment, the way a deployment configures one,
 * for the mode `WEBHOOK_RECEIVER_MODE` names ({@link modeFromEnv}).
 *
 * # `sealed`
 *
 * - `WEBHOOK_RECEIVER_SERVER_KEYS_PATH`: a file holding `sms-gateway
 *   server-keys`' output (`current: …` / `next: …`), or
 *   `WEBHOOK_RECEIVER_SERVER_CURRENT` and `WEBHOOK_RECEIVER_SERVER_NEXT`: the
 *   two public keys as unpadded base64url. They are public; this is plain
 *   configuration, not a secret store.
 * - `WEBHOOK_RECEIVER_AUDIENCE`: the audience the `WebhookEndpoint` was
 *   created with (default {@link DEFAULT_AUDIENCE}). Choose one no one else
 *   can have: the gateway hands each audience to one endpoint, and your origin
 *   is a good one.
 * - `WEBHOOK_RECEIVER_ENDPOINT_ID`: the endpoint's own id
 *   (`WebhookEndpoint.id`; the console shows it when the endpoint is created).
 *   Required, and the binding: one gateway key signs every app's deliveries, a
 *   global role may register an endpoint under any app, and so only the
 *   endpoint id, which no one but the gateway chooses, says a delivery was made
 *   for this receiver. A receiver serves exactly one endpoint.
 * - `WEBHOOK_RECEIVER_APP_ID`: the id of the vsms app the endpoint belongs to
 *   (`WebhookEndpoint.appId`). Required. It additionally catches an endpoint
 *   registered under another app; the opener compares it with the `appId`
 *   inside the signed envelope.
 *
 * A receiver that cannot name the gateway's keys, its own app or its own
 * endpoint refuses to start: there is no "accept anything" mode.
 *
 * # `hmac`
 *
 * - `WEBHOOK_RECEIVER_SECRET`: the endpoint's `secret`. Required. This one **is**
 *   a secret: read it from wherever you keep secrets, never from source.
 * - `WEBHOOK_RECEIVER_PREV_SECRET`: the endpoint's `prevSecret`, set only while a
 *   rotation's overlap window is open (`rotateWebhookSecret`), so a delivery
 *   signed with either verifies. Optional.
 *
 * A receiver with no secret refuses to start.
 */
export function openerFromEnv(
  env: NodeJS.ProcessEnv = process.env,
  nowSeconds?: number,
): EventOpener {
  if (modeFromEnv(env) === "hmac") {
    const prev = env.WEBHOOK_RECEIVER_PREV_SECRET;
    return new HmacOpener(
      required(env, "WEBHOOK_RECEIVER_SECRET"),
      prev === undefined || prev === "" ? undefined : prev,
    );
  }
  const path = env.WEBHOOK_RECEIVER_SERVER_KEYS_PATH;
  const serverKeys =
    path !== undefined && path !== ""
      ? parseServerKeysReport(readFileSync(path, "utf8"))
      : {
          current: required(env, "WEBHOOK_RECEIVER_SERVER_CURRENT"),
          next: required(env, "WEBHOOK_RECEIVER_SERVER_NEXT"),
        };
  return sealedOpener(
    new WebhookOpener({
      serverKeys,
      audience: env.WEBHOOK_RECEIVER_AUDIENCE ?? DEFAULT_AUDIENCE,
      route: ROUTE,
      appId: required(env, "WEBHOOK_RECEIVER_APP_ID"),
      endpointId: required(env, "WEBHOOK_RECEIVER_ENDPOINT_ID"),
      nowSeconds,
    }),
  );
}

const MISSING: Record<string, string> = {
  WEBHOOK_RECEIVER_SECRET:
    "WEBHOOK_RECEIVER_SECRET is required in hmac mode: the endpoint's signing secret " +
    "(`WebhookEndpoint.secret`). The receiver has no mode that accepts an unsigned delivery",
  WEBHOOK_RECEIVER_APP_ID:
    "WEBHOOK_RECEIVER_APP_ID is required: the id of the vsms app the endpoint belongs to. The " +
    "gateway signs every app's deliveries with one key, so the receiver has no mode that accepts " +
    "a delivery without knowing whose it is",
  WEBHOOK_RECEIVER_ENDPOINT_ID:
    "WEBHOOK_RECEIVER_ENDPOINT_ID is required: the id of the endpoint this receiver serves " +
    "(the console shows it when the endpoint is created). It is the binding between a delivery " +
    "and this receiver; the receiver has no mode that accepts a delivery without it",
  WEBHOOK_RECEIVER_SERVER_CURRENT: serverKeysMissing("WEBHOOK_RECEIVER_SERVER_CURRENT"),
  WEBHOOK_RECEIVER_SERVER_NEXT: serverKeysMissing("WEBHOOK_RECEIVER_SERVER_NEXT"),
};

function serverKeysMissing(name: string): string {
  return (
    `${name} is required (or WEBHOOK_RECEIVER_SERVER_KEYS_PATH): the receiver pins the ` +
    "gateway's public keys from `sms-gateway server-keys` and has no mode that skips them"
  );
}

function required(env: NodeJS.ProcessEnv, name: string): string {
  const value = env[name];
  if (value === undefined || value === "") {
    throw new Error(MISSING[name] ?? `${name} is required`);
  }
  return value;
}
