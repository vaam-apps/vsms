import { createRequire } from "node:module";
import path from "node:path";
import { defineConfig } from "vitest/config";

// `server-only` throws unconditionally unless resolved under the
// `react-server` export condition, which only Next's own webpack config
// sets up. Outside Next (i.e. here, under plain Vitest/Node), alias it to
// its own `empty.js` — the same no-op every server-only module gets when
// legitimately imported from a server context — rather than stripping the
// `import "server-only"` guard from source files just to make them
// testable. Resolved via `require.resolve` rather than a hand-written
// relative path: pnpm's own layout for this package is a symlink whose
// exact target depth isn't worth hard-coding.
const require = createRequire(import.meta.url);
// `server-only`'s own `exports` map doesn't expose `./package.json` as a
// subpath, so resolve its main entry (`index.js`, the throwing one) and
// take its directory instead.
const serverOnlyDir = path.dirname(require.resolve("server-only"));

export default defineConfig({
  resolve: {
    alias: {
      "server-only": path.join(serverOnlyDir, "empty.js"),
    },
  },
  test: {
    // `messages.ts` imports `@vsms/env` at module scope, which validates
    // every required var (`SMS_API_URL` etc.) at import time — same
    // `SKIP_ENV_VALIDATION` escape hatch `@vsms/env` documents for CI
    // builds with no real upstream configured. These tests inject
    // `fetchWindow` directly and never exercise the real HTTP path, so
    // the underlying values being unset is fine; only import-time
    // validation needs silencing.
    //
    // `SMS_API_URL` is set to a real (if fake) value, not left unset: #59's
    // `rest.test.ts` injects a fake low-level fetcher, but `restUrl()`
    // (`rest.ts`) still resolves every request path against
    // `env.SMS_API_URL` with `new URL(path, env.SMS_API_URL)` before that
    // fetcher ever runs — an unset base throws immediately, since a bare
    // path like `/providers/id` isn't a valid absolute URL on its own.
    // `http:`, not `https:`, so `dispatcher.ts`'s `buildAgent()` (still
    // called for its `dispatcher` option, even though the fake fetcher
    // ignores it) takes its no-certs-required branch.
    env: { SKIP_ENV_VALIDATION: "true", SMS_API_URL: "http://sms-api.test" },
  },
});
