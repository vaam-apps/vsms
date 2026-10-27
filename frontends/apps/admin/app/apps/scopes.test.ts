import { describe, expect, it } from "vitest";
import { KNOWN_SCOPES, parseScopes, serializeScopes } from "./scopes";

describe("the scope picker", () => {
  it("never offers console:pair: only the operator's command line may grant it", () => {
    // The gateway refuses it on POST and PATCH /app_clients and in
    // provisionAppClient for every caller (backends/crates/sms-api/src/
    // pair_scope.md), so a tick box for it would always fail.
    expect(KNOWN_SCOPES as readonly string[]).not.toContain("console:pair");
    expect(parseScopes("sms:send console:pair")).toEqual(["sms:send"]);
    expect(serializeScopes(["sms:send"])).not.toContain("console:pair");
  });
});
