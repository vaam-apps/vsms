// @vsms/api — the tRPC `appRouter` and its context.
//
// The router runs in the BROWSER. It used to run in the console's server, which
// held a credential and called the gateway on the operator's behalf; now the
// operator's own key signs every call, so the code that decides what to call
// has to be where the key is. `@vsms/hooks` mounts it behind tRPC's own fetch
// adapter on an in-process `fetch`, so the wire protocol (batching, superjson,
// the error formatter that carries a validation error's `fieldErrors`) is
// unchanged and no route handler stands between a screen and the gateway data
// layer.
//
// Nothing in this package may import Node (`node:*`, `server-only`,
// `@vsms/env`): it is bundled for the page. The gateway's server half is
// `@vsms/gateway/bff`, and only the console's route handlers import it.

export type { Context } from "./context";
export { createContext } from "./context";
export type { AppRouter } from "./router";
export { appRouter } from "./router";
