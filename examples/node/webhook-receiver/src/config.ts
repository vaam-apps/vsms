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
 *   created with (default {@link DEFAULT_AUDIENCE}).
 *
 * A receiver that cannot name the gateway's keys refuses to start: there is no
 * "accept anything" mode.
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
    nowSeconds,
  });
}

function required(env: NodeJS.ProcessEnv, name: string): string {
  const value = env[name];
  if (value === undefined || value === "") {
    throw new Error(
      `${name} is required (or WEBHOOK_RECEIVER_SERVER_KEYS_PATH): the receiver pins the ` +
        "gateway's public keys from `sms-gateway server-keys` and has no mode that skips them",
    );
  }
  return value;
}
