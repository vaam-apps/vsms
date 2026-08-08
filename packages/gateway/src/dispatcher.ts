import "server-only";

// The transport identity to sms-api. mTLS proves which *process* is
// calling (zero authorisation on its own — see token.ts for the Bearer
// that proves *which principal*). Per the architecture plan §4/DECISIONS,
// there is no `MTLS_ENABLED` flag: the scheme of `SMS_API_URL` selects the
// branch, and `@vsms/env`'s own boot-time cross-field rules already refuse
// to start with an inconsistent combination (`https:` without all three
// cert paths, or `http:` with any of them set). By the time this module
// runs, the env is already known-consistent.
//
// Three traps, all found the hard way (see architecture plan §4 / T4):
//
// 1. Never hand `dispatcher` to Next's own global `fetch`. Next patches
//    the global `fetch`, the patched function silently drops an unknown
//    `dispatcher` option, and the request still "succeeds" — just without
//    a client certificate. That is a fail-open bug in an mTLS control, not
//    a crash, which is exactly why it is dangerous: nothing here would
//    make it loud. `client.ts` and `token.ts` both import
//    `{ fetch as undiciFetch } from "undici"` explicitly and never touch
//    the ambient global.
// 2. Cache the `Agent` on `globalThis`, not a module-level `const`. Next's
//    dev server re-evaluates modules on every HMR edit; a module-level
//    `const` would rebuild (and leak) a fresh TLS connection pool per
//    edit until the process runs out of sockets. `globalThis` survives
//    HMR the same way a cached Prisma client conventionally does.
// 3. Certificates are read from disk lazily, at first use, inside this
//    `server-only` module — never through `NEXT_PUBLIC_*`, which would
//    ship key material into the client bundle.
//
// `sms-gateway` serves both the OP's `/token` route (token.ts) and the
// procedure routes (client.ts) from the same process and origin in every
// deployment this repo defines (see `.env.example`: `SMS_AUTH_ISSUER` and
// `SMS_API_URL` are the same address in dev, and nothing in `@vsms/env`
// models them as separately configurable transports) — so one `Agent`,
// built from `SMS_API_URL`'s scheme, serves both callers.

import { readFileSync } from "node:fs";
import { env } from "@vsms/env";
import { Agent } from "undici";

declare global {
  // eslint-disable-next-line no-var
  var __vsmsGatewayAgent: Agent | undefined;
}

function buildAgent(): Agent {
  const apiUrl = new URL(env.SMS_API_URL);

  if (apiUrl.protocol !== "https:") {
    // Dev default: plain HTTP loopback, matching the honest label in the
    // architecture plan §4 — mTLS is built, but only this branch is
    // verified against a real server today. `@vsms/env` already refused
    // to boot if any cert path were set alongside an `http:` URL, so
    // reaching here means none are.
    return new Agent({ keepAliveTimeout: 30_000 });
  }

  // `@vsms/env`'s own cross-field rule guarantees all three paths are set
  // whenever the URL is `https:` — see packages/env/src/index.ts.
  const certPath = env.SMS_API_CLIENT_CERT_PATH;
  const keyPath = env.SMS_API_CLIENT_KEY_PATH;
  const caPath = env.SMS_API_CA_PATH;
  if (certPath == null || keyPath == null || caPath == null) {
    throw new Error(
      "SMS_API_URL uses https: but a cert path is missing at runtime — this should have been " +
        "caught by @vsms/env's boot-time validation; the environment changed after startup",
    );
  }

  return new Agent({
    connect: {
      key: readFileSync(keyPath),
      cert: readFileSync(certPath),
      ca: readFileSync(caPath),
      minVersion: "TLSv1.2",
    },
    keepAliveTimeout: 30_000,
  });
}

/**
 * The process-wide `undici.Agent` used for every call to sms-api —
 * `/token` (token.ts) and `/$procs/*` (client.ts) alike. Cached on
 * `globalThis` so dev-mode HMR reuses the same TLS/keep-alive pool instead
 * of leaking one per edit (trap 2 above).
 */
export function gatewayAgent(): Agent {
  globalThis.__vsmsGatewayAgent ??= buildAgent();
  return globalThis.__vsmsGatewayAgent;
}
