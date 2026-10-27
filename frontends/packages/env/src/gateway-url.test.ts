import { describe, expect, it } from "vitest";
import { gatewayUrlProblem } from "./gateway-url";

describe("gatewayUrlProblem", () => {
  it("accepts an origin, with or without the trailing slash", () => {
    for (const url of [
      "http://sms-gateway:8080",
      "http://sms-gateway:8080/",
      "https://api.example.com",
      "https://127.0.0.1:8443/",
    ]) {
      expect(gatewayUrlProblem(url), url).toBeUndefined();
    }
  });

  it("refuses a path prefix, because every request path would replace it", () => {
    for (const url of [
      "https://edge.example.com/vsms",
      "https://edge.example.com/vsms/",
      "http://gw:8080/api",
    ]) {
      expect(gatewayUrlProblem(url), url).toMatch(/no path/);
    }
  });

  it("refuses a query, a fragment, credentials and other schemes", () => {
    expect(gatewayUrlProblem("http://gw:8080/?x=1")).toMatch(/no path/);
    expect(gatewayUrlProblem("http://gw:8080/#x")).toMatch(/no path/);
    expect(gatewayUrlProblem("http://user:pw@gw:8080")).toMatch(/credentials/);
    expect(gatewayUrlProblem("ftp://gw")).toMatch(/http: or https:/);
    expect(gatewayUrlProblem("not a url")).toMatch(/not a URL/);
  });
});
