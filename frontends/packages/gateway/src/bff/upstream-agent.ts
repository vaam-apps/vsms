import "server-only";

// The console's transport identity to the gateway, as a function the
// pass-through and the composer both call (`Upstream`). It is not an
// authentication: mTLS proves which *process* is calling, the sealed request
// proves which *key* signed it, and the gateway needs the second whatever the
// first says.
//
// The scheme of the gateway's URL selects the branch (there is no flag):
// `https:` presents the client certificate and checks the server's against the
// CA the deployment names, `http:` is the loopback of a development stack.
// `@vsms/env`'s own boot-time rules already refuse an inconsistent
// combination. Node's `http(s).request` rather than `fetch`: a sealed `GET`
// carries its message as the request body, which `fetch` refuses, and a redirect
// must never be followed with a signed body (`maxRedirects` is not a thing here:
// nothing is followed, a 3xx is returned as the answer it is).

import { readFileSync } from "node:fs";
import http from "node:http";
import https from "node:https";
import type { Upstream, UpstreamAnswer } from "./pass-through";
import { MAX_RESPONSE_BYTES } from "./pass-through";

export interface GatewayAddress {
  /** The gateway's origin, e.g. `https://sms-gateway:8080`. */
  url: string;
  /** All three, or none. */
  clientCertPath?: string | undefined;
  clientKeyPath?: string | undefined;
  caPath?: string | undefined;
  /** Per request, milliseconds. */
  timeoutMs?: number | undefined;
}

/** An agent for `address`, reading the certificates once. */
export function agentFor(address: GatewayAddress): http.Agent | https.Agent {
  const base = new URL(address.url);
  if (base.protocol !== "https:") return new http.Agent({ keepAlive: true });
  if (!address.clientCertPath || !address.clientKeyPath || !address.caPath) {
    throw new Error(
      "the gateway URL uses https: but a certificate path is missing: all of the client " +
        "certificate, its key and the CA must be given",
    );
  }
  return new https.Agent({
    key: readFileSync(address.clientKeyPath),
    cert: readFileSync(address.clientCertPath),
    ca: readFileSync(address.caPath),
    minVersion: "TLSv1.2",
    keepAlive: true,
  });
}

/** The agent for `address`, built on first use and kept: the certificates are
 *  read from disk when a request needs them, never at import time (a build has
 *  none). */
export function createUpstream(address: GatewayAddress): Upstream {
  let agent: http.Agent | https.Agent | undefined;
  const base = new URL(address.url);
  const timeoutMs = address.timeoutMs ?? 30_000;

  return (request) => {
    agent ??= agentFor(address);
    const target = new URL(request.target, base);
    // The path is the gateway's, whatever prefix its URL carries.
    if (target.origin !== base.origin) {
      return Promise.reject(new Error("the target is not on the gateway's origin"));
    }
    const transport = base.protocol === "https:" ? https : http;
    return new Promise<UpstreamAnswer>((resolve, reject) => {
      const outgoing = transport.request(
        target,
        {
          method: request.method,
          agent,
          timeout: timeoutMs,
          headers: Object.fromEntries([
            ...request.headers,
            ["content-length", String(request.body.byteLength)],
          ]),
        },
        (incoming) => {
          const chunks: Buffer[] = [];
          let total = 0;
          incoming.on("data", (chunk: Buffer) => {
            total += chunk.byteLength;
            if (total > MAX_RESPONSE_BYTES) {
              incoming.destroy(new Error("the answer is too large"));
              return;
            }
            chunks.push(chunk);
          });
          incoming.on("error", reject);
          incoming.on("end", () => {
            const headers = new Headers();
            for (const [name, value] of Object.entries(incoming.headers)) {
              if (typeof value === "string") headers.set(name, value);
              else if (Array.isArray(value)) headers.set(name, value.join(", "));
            }
            resolve({
              status: incoming.statusCode ?? 502,
              headers,
              body: new Uint8Array(Buffer.concat(chunks)),
            });
          });
        },
      );
      outgoing.on("timeout", () => outgoing.destroy(new Error("the gateway timed out")));
      outgoing.on("error", reject);
      outgoing.end(Buffer.from(request.body));
    });
  };
}
