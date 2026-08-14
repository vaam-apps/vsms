import { describe, expect, it } from "vitest";

import { LOGIN_ERROR_MESSAGES, loginErrorMessage, UNKNOWN_LOGIN_ERROR } from "./login-errors";

describe("loginErrorMessage", () => {
  it("returns undefined when there is no error, so nothing renders", () => {
    expect(loginErrorMessage(undefined)).toBeUndefined();
  });

  it("maps every known code to its own message", () => {
    for (const [code, message] of Object.entries(LOGIN_ERROR_MESSAGES)) {
      expect(loginErrorMessage(code)).toBe(message);
    }
  });

  /**
   * The case the module doc calls load-bearing: `/api/auth/login` can
   * redirect with a code this table has never seen, and falling through to
   * `undefined` would render no alert at all — telling the user their
   * password worked when it did not.
   */
  it("falls back rather than rendering nothing for an unknown code", () => {
    expect(loginErrorMessage("some_code_added_later")).toBe(UNKNOWN_LOGIN_ERROR);
    expect(loginErrorMessage("")).toBe(UNKNOWN_LOGIN_ERROR);
  });

  /** Next hands a repeated query parameter back as an array. */
  it("takes the first value when the param is repeated", () => {
    expect(loginErrorMessage(["expired", "invalid_request"])).toBe(LOGIN_ERROR_MESSAGES.expired);
  });

  it("still falls back when a repeated param's first value is unknown", () => {
    expect(loginErrorMessage(["nope", "expired"])).toBe(UNKNOWN_LOGIN_ERROR);
  });
});
