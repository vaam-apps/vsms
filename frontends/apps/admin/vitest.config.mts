import { defineConfig } from "vitest/config";

// Two independent sets of pure unit tests live under `frontends/apps/admin/` now, and
// neither needs the escape hatches `frontends/packages/gateway/vitest.config.ts`
// carries:
//
//   - `lib/oidc.ts` (#194) is pure — no `@vsms/env` import at module
//     scope, no `server-only` guard.
//   - `app/messages/[id]/timeline.ts` (#50) needs `@vsms/ui`'s
//     `StateTransition`, but only ever as `import type`, which Vitest's
//     esbuild transform erases entirely — so no React or Next runtime is
//     pulled in either.
//
// `include` is widened to cover both locations: Vitest's default globs
// would miss `app/**`, which is where #50's tests live. Kept as its own
// file rather than folded into a shared root config because this
// workspace has no shared vitest config today.
export default defineConfig({
  // Next requires `jsx: "preserve"` in tsconfig.json, and from Vite 8 the
  // transform honours that — so a test importing a `.tsx` component gets
  // unprocessed JSX handed to the parser and fails with "content contains
  // invalid JS syntax ... make sure to not set jsx to preserve". This
  // overrides the tsconfig for the test transform only, leaving Next's
  // own requirement untouched.
  //
  // `oxc`, not `esbuild`: Vite 8 transforms with Oxc, and the `esbuild`
  // key is silently ignored — setting it changes nothing and the same
  // parse error comes back, which is how this took two attempts.
  oxc: { jsx: { runtime: "automatic" } },
  test: {
    include: ["app/**/*.test.ts", "lib/**/*.test.ts"],
  },
});
