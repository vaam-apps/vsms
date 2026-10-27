/**
 * The gateway's address, parsed once, and the URL a sealed call is sent to.
 *
 * The signature binds the route and its parameters; the URL is what actually
 * reaches the gateway. They must say the same thing, or a request is signed for
 * one place and sent to another (the gateway then refuses it as the coarse
 * `401`, which is safe but says nothing about why). So the base is held to an
 * origin and an optional path prefix, nothing a URL parser could re-read as part
 * of the route (a query, a fragment), and nothing that makes Node add a header
 * of its own (userinfo becomes `Authorization: Basic`, a shared secret this
 * keypair-only SDK never sends). And the URL built for a call is checked
 * against the string it was built from before anything is sent.
 */
import { SdkError } from "./error.js";

/** A parsed gateway address. */
export interface GatewayBase {
  /** `scheme://host[:port]`, normalised. */
  readonly origin: string;
  /** The path prefix with every trailing `/` removed: `""` for a gateway at the root. */
  readonly basePath: string;
}

/**
 * `path` with every trailing `/` removed. A loop rather than `/\/+$/`:
 * that pattern backtracks quadratically on a string of many `/` that does
 * not end in one (CodeQL js/polynomial-redos), and this runs on caller input.
 */
export function withoutTrailingSlashes(path: string): string {
  let end = path.length;
  while (end > 0 && path.charCodeAt(end - 1) === 0x2f) end -= 1;
  return path.slice(0, end);
}

function config(message: string, cause?: unknown): SdkError {
  return new SdkError(message, { kind: "config", cause });
}

/**
 * Parses `baseUrl`: `http:` or `https:`, a host, an optional path prefix, and
 * nothing else. The text is never quoted in a message, since a URL may carry a
 * secret it should not.
 */
export function parseBaseUrl(baseUrl: unknown): GatewayBase {
  if (typeof baseUrl !== "string" || baseUrl.length === 0) {
    throw config("baseUrl is required");
  }
  // Checked on the text: a URL with an empty `?` or `#` parses with an empty
  // `search` or `hash`, indistinguishable from none, and still changes where a
  // string appended to it lands.
  if (baseUrl.includes("?") || baseUrl.includes("#")) {
    throw config("baseUrl must be an origin and an optional path: no query string and no fragment");
  }
  let url: URL;
  try {
    url = new URL(baseUrl);
  } catch (cause) {
    throw config("baseUrl is not a valid URL", cause);
  }
  if (url.protocol !== "http:" && url.protocol !== "https:") {
    throw config("baseUrl must be an http: or https: URL");
  }
  if (url.username !== "" || url.password !== "") {
    throw config(
      "baseUrl must not carry credentials: Node would send them as a Basic Authorization " +
        "header, and this SDK authenticates by signature alone",
    );
  }
  return { origin: url.origin, basePath: withoutTrailingSlashes(url.pathname) };
}

/**
 * The URL to send a sealed call to: the gateway's origin, its path prefix and
 * the `target` the sealing module built from the parameters it bound.
 *
 * Refuses (a `seal` error, before anything is sent) unless parsing that string
 * gives back exactly the path and query it was built from, on the same origin
 * and with no fragment and no credentials. A target the URL parser would
 * rewrite, resolve or cut short is not the target that was signed.
 */
export function sealedUrl(base: GatewayBase, target: string): string {
  const expected = `${base.basePath}${target}`;
  let url: URL;
  try {
    url = new URL(`${base.origin}${expected}`);
  } catch (cause) {
    throw new SdkError("the signed target is not a URL", { kind: "seal", cause });
  }
  if (
    url.origin !== base.origin ||
    url.username !== "" ||
    url.password !== "" ||
    url.hash !== "" ||
    `${url.pathname}${url.search}` !== expected
  ) {
    throw new SdkError("the signed target does not survive URL parsing unchanged", {
      kind: "seal",
    });
  }
  return url.href;
}
