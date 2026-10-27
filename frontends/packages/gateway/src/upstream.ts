// The one door every module of this package calls the gateway through.
//
// The data layer runs in the browser (see `index.ts`): the person's signing key
// lives there, as a non-extractable WebCrypto key, and a request is only worth
// anything to the gateway once that key has signed it. So a module here never
// holds a credential, a cookie or a URL of the gateway. It writes the request
// it wants (`GET /apps?limit=500`) against a placeholder origin, and whatever
// `configureGatewayFetch` was given turns it into a sealed request, sends it
// and hands back what the sealed answer said, as an ordinary `Response`. In
// the console that is `createSealingFetch` (`./sealing-fetch.ts`); in a test it
// is a fake.
//
// There is deliberately no default: a call made before the console has
// configured a transport fails loudly instead of reaching for some other way to
// authenticate.

/** The origin every request is written against. It is never contacted: only
 *  the path and query are read, and the sealed request goes where the
 *  transport sends it. `.invalid` is reserved (RFC 2606) and resolves nowhere,
 *  so a transport that forgot to rewrite it fails instead of connecting. */
export const GATEWAY_ORIGIN = "https://gateway.invalid";

/** `fetch`, as far as this package uses it. */
export type GatewayFetch = (input: string | URL, init?: RequestInit) => Promise<Response>;

/** A call was made before a transport was configured. */
export class GatewayNotConfiguredError extends Error {
  constructor() {
    super("the gateway transport is not configured: no sealing fetch was installed");
    this.name = "GatewayNotConfiguredError";
  }
}

let transport: GatewayFetch | undefined;

/** Installs (or, with `undefined`, removes) the transport. The console does
 *  this once, before anything renders. */
export function configureGatewayFetch(next: GatewayFetch | undefined): void {
  transport = next;
}

/** Sends one request to the gateway through the configured transport. */
export function gatewayFetch(input: string | URL, init?: RequestInit): Promise<Response> {
  if (transport === undefined) return Promise.reject(new GatewayNotConfiguredError());
  return transport(input, init);
}

/** `path` and `query` as a URL on the placeholder origin. Undefined query
 *  values are left out; the rest are stringified. */
export function gatewayUrl(
  path: string,
  query: Record<string, string | number | undefined> = {},
): string {
  const url = new URL(path, GATEWAY_ORIGIN);
  for (const [key, value] of Object.entries(query)) {
    if (value !== undefined) url.searchParams.set(key, String(value));
  }
  return url.toString();
}

/** The composer's transport: `preview` and `send` go through the console's own
 *  service key on its server, not through the person's key (see
 *  `./compose-proof.ts`). */
export type ComposeFetch = (operation: "preview" | "send", input: unknown) => Promise<Response>;

let composer: ComposeFetch | undefined;

/** Installs (or removes) the composer's transport. */
export function configureComposeFetch(next: ComposeFetch | undefined): void {
  composer = next;
}

/** One composer call through the configured transport. */
export function composeFetch(operation: "preview" | "send", input: unknown): Promise<Response> {
  if (composer === undefined) return Promise.reject(new GatewayNotConfiguredError());
  return composer(operation, input);
}
