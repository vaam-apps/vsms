// The proof a browser attaches to a composer call.
//
// The composer sends through the console's own service key, held by the
// console's server (`./bff/composer.ts`), because `previewMessage` and
// `sendMessage` are procedures only an app may call. That key is a standing
// authority, so the server must not use it for whoever happens to reach the
// route. It uses it for a browser the gateway has just vouched for: the browser
// seals one cheap read with its own key, the server forwards those bytes, and
// only a 200 from the gateway (which accepts a sealed request from a paired,
// unexpired, unrevoked console key and nothing else) lets the call through.
// There is no session and no cookie: the proof is a request, it is single-use
// (the gateway refuses a nonce it has seen) and it is five minutes old at most.
//
// `GET /sender_ids?fields=id&limit=1` is the read: `SenderId`'s policy admits
// any `user`, the row set is small, and `fields=id` keeps the answer to ids.
// Both halves, the browser that seals it and the server that checks what it was
// sent, import these three constants so the route is written once.

/** The route of the proof read. */
export const COMPOSE_PROOF_PATH = "/sender_ids";
/** Its query, in the order it is sent. */
export const COMPOSE_PROOF_QUERY = "fields=id&limit=1";
/** The target a sealed proof must name. */
export const COMPOSE_PROOF_TARGET = `${COMPOSE_PROOF_PATH}?${COMPOSE_PROOF_QUERY}`;
