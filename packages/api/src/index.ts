import "server-only";

// @vsms/api — the tRPC `appRouter` and its Next.js route-handler context.
// server-only: this package's `context.ts` imports `@vsms/gateway`, which
// reads certificate/key material from disk. `@vsms/hooks` must only ever
// `import type { AppRouter }` from here (verbatimModuleSyntax makes a
// value import a compile error) — a value import would drag this whole
// module graph, `server-only`'s own guard included, into the browser
// bundle.

export type { Context } from "./context";
export { createContext } from "./context";
export type { AppRouter } from "./router";
export { appRouter } from "./router";
