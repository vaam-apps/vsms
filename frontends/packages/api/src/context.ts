// The router's context. The router runs in the browser (see `index.ts`), so
// what a procedure may reach is exactly what the browser's own code already can:
// the gateway data layer, which seals every call with the browser's key, and the
// operational tuning values the console's server read from its environment and
// handed down.
//
// There is no actor, no origin check and no session here, deliberately. Those
// guarded a server that held a credential and spent it on behalf of whoever
// reached it (a cross-site POST could have made the console send an SMS as the
// operator). Nothing here spends anything: a request leaves this process only
// once the browser's key has signed it, and a page that can make the key sign
// can already do anything this router could.

import * as gateway from "@vsms/gateway";

export interface Context {
  gateway: typeof gateway;
  /** How long the Messages screen's state-change poll waits for a frame
   *  (`MESSAGE_STREAM_POLL_MS`, read by the server and handed down). */
  messageStreamPollMs: number;
}

/** The context for a router running in the page. */
export function createContext(options: { messageStreamPollMs: number }): Context {
  return { gateway, messageStreamPollMs: options.messageStreamPollMs };
}
