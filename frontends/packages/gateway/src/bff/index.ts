import "server-only";

// @vsms/gateway/bff — the console's server half: a pass-through of sealed bytes
// and the composer's service key. See `./pass-through.ts` and `./composer.ts`.

export type { ComposerClient, ComposerOpener, ComposerOptions } from "./composer";
export { createComposer } from "./composer";
export type { Upstream, UpstreamAnswer, UpstreamRequest } from "./pass-through";
export {
  COSE_SIGN1,
  createPassThrough,
  FORWARDED_REQUEST_HEADERS,
  FORWARDED_RESPONSE_HEADERS,
  MAX_REQUEST_BYTES,
  MAX_RESPONSE_BYTES,
  METHODS,
  pick,
  upstreamTarget,
} from "./pass-through";
export type { GatewayAddress } from "./upstream-agent";
export { agentFor, createUpstream } from "./upstream-agent";
