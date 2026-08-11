import { defineConfig } from "vitest/config";

// admin/lib/oidc.ts is pure — no @vsms/env import at module scope, no
// server-only guard — so, unlike @vsms/gateway's own vitest.config.ts,
// nothing here needs an env-validation escape hatch or a server-only
// alias. Kept as its own file (not folded into a shared root config)
// because this workspace has no shared vitest config today and inventing
// one is out of scope for #194.
export default defineConfig({
  test: {},
});
