// #243: every console mutation returned FORBIDDEN in production.
//
// `assertSameOriginForMutations` used to compare the browser's `Origin`
// against `new URL(req.url).origin`. That is correct only when Next.js
// runs as a host process, where the bind address and the browser's origin
// happen to coincide — which is exactly what `just demo` does, and why
// this survived undetected.
//
// In a container it is never correct. Measured against a real Next.js
// 15.5.23 standalone server behind a reverse proxy (the same topology as
// `deploy/docker-compose.yml`), `req.url` comes back as
// `https://0.0.0.0:3000/...`: the host is the server's own
// `HOSTNAME`/`PORT` bind, and neither `Host` nor `X-Forwarded-Host`
// influences it. No browser can send that as an `Origin`.
//
// The first test below is the regression guard, and it is only meaningful
// because of the shape of the fix. Under the old implementation, *no*
// unit test of this function could have failed — the function faithfully
// compared two values, and the bug was that one of its inputs meant
// something different in a container than on a host. Pinning the expected
// origin to `ADMIN_BASE_URL` is what makes the container case expressible
// here at all: `requestUrl` below is deliberately the container bind
// address, and the test asserts the call still succeeds.

import { beforeEach, describe, expect, it, vi } from "vitest";

const ADMIN_BASE_URL = "https://console.example.com";

vi.mock("@vsms/env", () => ({
  env: {
    ADMIN_BASE_URL,
    // `createContext` pulls in `@vsms/gateway`, which reads several of
    // these at module scope. Values are irrelevant to this file — only
    // their presence is.
    SMS_API_URL: "http://sms-gateway:8080",
    SMS_AUTH_ISSUER: "https://sms.example.com",
    SMS_CONSOLE_CLIENT_ID: "console",
    SMS_CONSOLE_SCOPE: "sms:read",
    SMS_CONSOLE_PRIVATE_KEY_PATH: "/secrets/key.pem",
    MESSAGE_STREAM_POLL_MS: 2000,
  },
}));

vi.mock("@vsms/gateway", () => ({}));

/** The URL a containerised Next.js standalone server actually reports —
 * see this file's own header. Not a hypothetical: this exact value was
 * observed from a probe running behind Caddy. */
const CONTAINER_REQUEST_URL = "https://0.0.0.0:3000/api/trpc/messages.send";

function post(headers: Record<string, string>, url = CONTAINER_REQUEST_URL): Request {
  return new Request(url, { method: "POST", headers });
}

describe("assertSameOriginForMutations", () => {
  let createContext: typeof import("./context").createContext;

  beforeEach(async () => {
    vi.resetModules();
    ({ createContext } = await import("./context"));
  });

  const ctx = (req: Request) =>
    // biome-ignore lint/suspicious/noExplicitAny: test helper providing partial options
    createContext({ req } as any);

  it("accepts a browser POST whose Origin matches ADMIN_BASE_URL, even though req.url is the container's bind address", () => {
    expect(() => ctx(post({ origin: ADMIN_BASE_URL }))).not.toThrow();
  });

  it("rejects a POST from a different origin", () => {
    expect(() => ctx(post({ origin: "https://evil.example.com" }))).toThrow(
      /cross-origin request rejected/,
    );
  });

  it("rejects a POST with no Origin header at all", () => {
    // Deliberate, and documented in `context.ts`'s own module doc: a
    // browser always sets `Origin` on a fetch POST, so a caller that omits
    // it is a non-browser client. This endpoint reaches `sendMessage`,
    // which sends a real billed SMS, so absence is refusal.
    expect(() => ctx(post({}))).toThrow(/requires an Origin header/);
  });

  it("does not check the origin of a GET", () => {
    expect(() => ctx(new Request(CONTAINER_REQUEST_URL, { method: "GET" }))).not.toThrow();
  });

  it("ignores a forged X-Forwarded-Host — the expected origin comes from configuration, not from a header", () => {
    // The alternative fix considered in #243 was honouring
    // `X-Forwarded-Host`. This asserts we did not do that: an attacker
    // who can set that header must not be able to redefine what counts
    // as same-origin.
    expect(() =>
      ctx(
        post({
          origin: "https://evil.example.com",
          "x-forwarded-host": "evil.example.com",
        }),
      ),
    ).toThrow(/cross-origin request rejected/);
  });
});
