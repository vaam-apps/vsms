import { describe, expect, it } from "vitest";
import { shouldRetryQuery } from "./retry";

const withStatus = (httpStatus: number) => ({ message: "x", data: { httpStatus } });

describe("shouldRetryQuery", () => {
  it("never retries the server's own 4xx answer", () => {
    for (const status of [400, 401, 403, 404, 409, 412, 429]) {
      expect(shouldRetryQuery(0, withStatus(status))).toBe(false);
    }
  });

  it("retries a 5xx and a network error up to three times", () => {
    expect(shouldRetryQuery(0, withStatus(502))).toBe(true);
    expect(shouldRetryQuery(2, new TypeError("fetch failed"))).toBe(true);
    expect(shouldRetryQuery(3, new TypeError("fetch failed"))).toBe(false);
  });
});
