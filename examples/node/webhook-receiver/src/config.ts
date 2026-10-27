import { readFileSync } from "node:fs";
import { parseServerKeysReport, WebhookOpener } from "@vaam-apps/vsms-node";

/** The route this receiver serves, and so the path of the `WebhookEndpoint.url` that points at it. */
export const ROUTE = "/webhooks/vsms";

/** The audience when none is configured: the name the example's endpoint is created with. */
export const DEFAULT_AUDIENCE = "webhook-receiver-example";

/**
 * Builds the opener from the environment, the way a deployment configures one.
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
 */
export function openerFromEnv(
  env: NodeJS.ProcessEnv = process.env,
  nowSeconds?: number,
): WebhookOpener {
  const path = env.WEBHOOK_RECEIVER_SERVER_KEYS_PATH;
  const serverKeys =
    path !== undefined && path !== ""
      ? parseServerKeysReport(readFileSync(path, "utf8"))
      : {
          current: required(env, "WEBHOOK_RECEIVER_SERVER_CURRENT"),
          next: required(env, "WEBHOOK_RECEIVER_SERVER_NEXT"),
        };
  return new WebhookOpener({
    serverKeys,
    audience: env.WEBHOOK_RECEIVER_AUDIENCE ?? DEFAULT_AUDIENCE,
    route: ROUTE,
    appId: required(env, "WEBHOOK_RECEIVER_APP_ID"),
    endpointId: required(env, "WEBHOOK_RECEIVER_ENDPOINT_ID"),
    nowSeconds,
  });
}

const MISSING: Record<string, string> = {
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
