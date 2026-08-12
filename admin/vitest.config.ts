import { defineConfig } from "vitest/config";

// #50: `app/messages/[id]/timeline.ts` is the first pure, unit-testable
// logic to live inside `admin/` itself rather than `packages/gateway` —
// it needs `@vsms/ui`'s `StateTransition` type (a UI-layer type, not a
// gateway one), and only ever as `import type`, which Vitest's esbuild
// transform erases entirely — no React/Next runtime is ever pulled in by
// these tests, so no special test environment or alias is needed here,
// unlike `packages/gateway/vitest.config.ts`'s `server-only` workaround.
export default defineConfig({
  test: {
    include: ["app/**/*.test.ts"],
  },
});
