import { fileURLToPath } from "node:url";
import type { NextConfig } from "next";

const config: NextConfig = {
  reactStrictMode: true,
  // The four `@vsms/*` entries are workspace packages whose `exports` point
  // straight at `./src/index.ts`, so Next has to compile them itself.
  // `@vaam-apps/ui` used to be listed here for exactly that reason and no
  // longer needs to be: since it left the workspace it arrives from npm as
  // pre-compiled ESM with an `exports` map and its `"use client"` prologues
  // already in place. Verified by removing it and diffing a real `next build`
  // against the previous one -- see the commit that made this change.
  transpilePackages: ["@vsms/env", "@vsms/gateway", "@vsms/api", "@vsms/hooks"],
  // Traces the actual runtime dependency graph into `.next/standalone`
  // (a minimal server.js plus only the node_modules it really touches),
  // so the Docker runtime stage (frontends/apps/admin/Dockerfile) doesn't need to carry
  // the full pnpm workspace's node_modules — verified against a real
  // `next build` and `node server.js`, not assumed. See frontends/apps/admin/Dockerfile's
  // own comment for the copy list this depends on.
  output: "standalone",
  // The static half of the console's hardening. A script that runs on a page of
  // this console can ask its key to sign (the key cannot be exported, but it can
  // be used while the page is open), so nothing on a page should leave it
  // uninvited: no referrer, no sniffed content types, no window sharing. A
  // per-request Content-Security-Policy (`script-src 'self' 'wasm-unsafe-eval'`
  // with a nonce) is the missing half and is called out in
  // docs/design/console-pairing.md, "What a stolen browser profile gets".
  //
  // Framing is the one thing a static policy settles: a page of this console
  // inside another site's frame is a page whose key an attacker can ask to sign
  // behind a transparent overlay. `frame-ancestors 'none'` is the modern header
  // and `X-Frame-Options: DENY` the one an older browser reads; neither
  // console route has a reason to be framed.
  async headers() {
    return [
      {
        source: "/:path*",
        headers: [
          { key: "X-Content-Type-Options", value: "nosniff" },
          { key: "Referrer-Policy", value: "no-referrer" },
          { key: "Cross-Origin-Opener-Policy", value: "same-origin" },
          { key: "X-Frame-Options", value: "DENY" },
          { key: "Content-Security-Policy", value: "frame-ancestors 'none'" },
          {
            key: "Permissions-Policy",
            value: "camera=(), microphone=(), geolocation=(), payment=()",
          },
        ],
      },
    ];
  },
  // Pin the trace root to the pnpm workspace root explicitly rather than
  // let Next infer it from the nearest lockfile. Without this, a checkout
  // nested under another pnpm workspace (this repo's own dev worktrees
  // included) makes Next pick the *outer* lockfile as the root, which
  // breaks `collect-build-traces` with a `MODULE_NOT_FOUND` on
  // `next/dist/server/route-modules/app-page/module.compiled` — found by
  // actually running `next build`, not from the warning text alone.
  outputFileTracingRoot: fileURLToPath(new URL("../../..", import.meta.url)),
};

export default config;
