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
  test: {},
});
