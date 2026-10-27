import { defineConfig } from "vitest/config";

// Nothing here needs an escape hatch: the router runs in the page, so it imports
// no `server-only`, no `@vsms/env` and no Node.
export default defineConfig({ test: {} });
