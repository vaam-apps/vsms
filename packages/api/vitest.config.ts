import { createRequire } from "node:module";
import path from "node:path";
import { defineConfig } from "vitest/config";

// Same `server-only` shim `packages/gateway/vitest.config.ts` documents at
// length: that module throws unconditionally unless resolved under the
// `react-server` export condition, which only Next's own webpack config
// sets up. Aliasing it to its own `empty.js` is what every server-only
// module legitimately gets in a server context — preferable to deleting
// the `import "server-only"` guard from source just to make it testable.
const require = createRequire(import.meta.url);
const serverOnlyDir = path.dirname(require.resolve("server-only"));

export default defineConfig({
  resolve: {
    alias: {
      "server-only": path.join(serverOnlyDir, "empty.js"),
    },
  },
  test: {
    // `context.test.ts` mocks `@vsms/env` outright, so no real
    // environment is needed here — unlike `packages/gateway`'s own
    // config, which has to supply a real `SMS_API_URL` because `restUrl()`
    // resolves against it before any injected fake runs.
    env: { SKIP_ENV_VALIDATION: "true" },
  },
});
