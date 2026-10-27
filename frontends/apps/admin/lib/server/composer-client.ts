import "server-only";

// The composer's service key, as a client of the gateway: an Ed25519 key the
// deployment registered with `sms-gateway register-client --client-name
// console-composer`, read from the file `SMS_COMPOSER_KEY_FILE` names when the
// composer first sends (a console deployed without one serves every other
// screen), signing every call through the Node SDK and believing only an answer
// that verifies under the gateway's two pinned keys.

import { AnswerOpener, VsmsClient } from "@vaam-apps/vsms-node";
import { env } from "@vsms/env";
import { agentFor } from "@vsms/gateway/bff";
import { gatewayAddress } from "./gateway-address";

let client: VsmsClient | undefined;

export function composerClient(): VsmsClient {
  if (client === undefined) {
    if (!env.SMS_COMPOSER_KEY_FILE) {
      throw new Error(
        "the composer has no service key: SMS_COMPOSER_KEY_FILE is not set " +
          '(docs/runbooks/deployment.adoc, "The composer\'s key")',
      );
    }
    client = new VsmsClient({
      baseUrl: env.SMS_API_URL,
      keyPath: env.SMS_COMPOSER_KEY_FILE,
      serverKeys: {
        current: env.SMS_ENVELOPE_SERVER_KEY_CURRENT,
        next: env.SMS_ENVELOPE_SERVER_KEY_NEXT,
      },
      agent: agentFor(gatewayAddress()),
    });
  }
  return client;
}

let opener: AnswerOpener | undefined;

/** Opens the gateway's answer to a browser's proof under the same two pinned
 *  keys the service key's client believes, so the composer takes the gateway's
 *  word and nobody else's for who is asking. */
export function composerOpener(): AnswerOpener {
  opener ??= new AnswerOpener({
    serverKeys: {
      current: env.SMS_ENVELOPE_SERVER_KEY_CURRENT,
      next: env.SMS_ENVELOPE_SERVER_KEY_NEXT,
    },
  });
  return opener;
}
