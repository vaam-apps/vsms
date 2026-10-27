import "server-only";

// Where the gateway is, as the console's server sees it (`@vsms/env`). One
// reader for the two route handlers that talk to it, so the certificate
// settings exist in one place.

import { env } from "@vsms/env";
import { createUpstream, type GatewayAddress, type Upstream } from "@vsms/gateway/bff";

export function gatewayAddress(): GatewayAddress {
  return {
    url: env.SMS_API_URL,
    clientCertPath: env.SMS_API_CLIENT_CERT_PATH,
    clientKeyPath: env.SMS_API_CLIENT_KEY_PATH,
    caPath: env.SMS_API_CA_PATH,
  };
}

let upstream: Upstream | undefined;

/** The one transport to the gateway, built on first use. Cached on the module:
 *  a Next dev server re-evaluates modules on every edit, and a pool per edit
 *  would leak sockets, so the route handlers share this and nothing else makes
 *  one. */
export function gatewayUpstream(): Upstream {
  upstream ??= createUpstream(gatewayAddress());
  return upstream;
}
