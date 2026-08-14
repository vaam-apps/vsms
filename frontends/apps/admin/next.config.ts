import { fileURLToPath } from "node:url";
import type { NextConfig } from "next";

const config: NextConfig = {
  reactStrictMode: true,
  transpilePackages: ["@vsms/env", "@vsms/ui", "@vsms/gateway", "@vsms/api", "@vsms/hooks"],
  // Traces the actual runtime dependency graph into `.next/standalone`
  // (a minimal server.js plus only the node_modules it really touches),
  // so the Docker runtime stage (frontends/apps/admin/Dockerfile) doesn't need to carry
  // the full pnpm workspace's node_modules — verified against a real
  // `next build` and `node server.js`, not assumed. See frontends/apps/admin/Dockerfile's
  // own comment for the copy list this depends on.
  output: "standalone",
  // Pin the trace root to the pnpm workspace root explicitly rather than
  // let Next infer it from the nearest lockfile. Without this, a checkout
  // nested under another pnpm workspace (this repo's own dev worktrees
  // included) makes Next pick the *outer* lockfile as the root, which
  // breaks `collect-build-traces` with a `MODULE_NOT_FOUND` on
  // `next/dist/server/route-modules/app-page/module.compiled` — found by
  // actually running `next build`, not from the warning text alone.
  outputFileTracingRoot: fileURLToPath(new URL("..", import.meta.url)),
};

export default config;
