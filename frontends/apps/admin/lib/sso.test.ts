import { describe, expect, it } from "vitest";

import {
  buildSsoStartUrl,
  loginErrorForOauthError,
  SSO_DISCOVERY_FIELD,
  ssoLoginEndpointFrom,
} from "./sso";

const GOOD = "https://vsms.example.org/login/sso/start";

describe("ssoLoginEndpointFrom", () => {
  it("reads the endpoint the gateway advertises", () => {
    expect(ssoLoginEndpointFrom({ [SSO_DISCOVERY_FIELD]: GOOD })).toBe(GOOD);
  });

  it("is undefined when SSO is not configured, so the button is not shown", () => {
    expect(ssoLoginEndpointFrom({ issuer: "https://vsms.example.org" })).toBeUndefined();
    expect(ssoLoginEndpointFrom(null)).toBeUndefined();
    expect(ssoLoginEndpointFrom("nope")).toBeUndefined();
    expect(ssoLoginEndpointFrom({ [SSO_DISCOVERY_FIELD]: 42 })).toBeUndefined();
  });

  it("refuses anything that is not a plain gateway start URL", () => {
    for (const bad of [
      "javascript:alert(1)",
      "https://vsms.example.org/somewhere/else",
      "https://vsms.example.org/login/sso/start?next=https://evil.example",
      "https://vsms.example.org/login/sso/start#frag",
      "https://user:pass@vsms.example.org/login/sso/start",
      "ftp://vsms.example.org/login/sso/start",
      "/login/sso/start",
      "",
    ]) {
      expect(ssoLoginEndpointFrom({ [SSO_DISCOVERY_FIELD]: bad }), bad).toBeUndefined();
    }
  });
});

describe("buildSsoStartUrl", () => {
  it("carries the console's whole authorize request, PKCE S256 included", () => {
    const url = new URL(
      buildSsoStartUrl(GOOD, {
        clientId: "sms-console",
        redirectUri: "https://admin.example.org/api/auth/callback",
        state: "the-state",
        nonce: "the-nonce",
        codeChallenge: "the-challenge",
      }),
    );
    expect(url.origin + url.pathname).toBe(GOOD);
    expect(Object.fromEntries(url.searchParams)).toEqual({
      client_id: "sms-console",
      redirect_uri: "https://admin.example.org/api/auth/callback",
      response_type: "code",
      scope: "openid profile",
      state: "the-state",
      nonce: "the-nonce",
      code_challenge: "the-challenge",
      code_challenge_method: "S256",
    });
  });

  it("encodes values rather than splicing them into the query", () => {
    const url = buildSsoStartUrl(GOOD, {
      clientId: "a&b=c",
      redirectUri: "https://admin.example.org/cb?x=1&y=2",
      state: "s s",
      nonce: "n",
      codeChallenge: "c",
    });
    const parsed = new URL(url);
    expect(parsed.searchParams.get("client_id")).toBe("a&b=c");
    expect(parsed.searchParams.get("redirect_uri")).toBe("https://admin.example.org/cb?x=1&y=2");
    expect(parsed.searchParams.get("y")).toBeNull();
  });
});

describe("loginErrorForOauthError", () => {
  it("maps the generic refusal and the outage to different generic codes", () => {
    expect(loginErrorForOauthError("access_denied")).toBe("sso_failed");
    expect(loginErrorForOauthError("temporarily_unavailable")).toBe("sso_unavailable");
    expect(loginErrorForOauthError("server_error")).toBe("sso_unavailable");
  });

  it("treats anything unknown as the generic refusal, never echoing it", () => {
    expect(loginErrorForOauthError("<script>")).toBe("sso_failed");
    expect(loginErrorForOauthError("")).toBe("sso_failed");
  });
});
