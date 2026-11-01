import { describe, expect, it } from "vitest";
import { KNOWN_SCOPES, parseScopes, SCOPE_DESCRIPTIONS, serializeScopes } from "./scopes";

describe("the scope picker", () => {
  it("offers delivery:read, so a key holding it keeps it through an edit", () => {
    // lookupDeliveryStatus checks it (require_permission); parseScopes drops what
    // the picker does not know, so a missing entry would strip it on the next save.
    expect(KNOWN_SCOPES as readonly string[]).toContain("delivery:read");
    expect(parseScopes(" sms:send delivery:read ")).toEqual(["sms:send", "delivery:read"]);
    expect(SCOPE_DESCRIPTIONS["delivery:read"]).toContain("without reading it");
  });

  it("never offers console:pair: only the operator's command line may grant it", () => {
    // The gateway refuses it on POST and PATCH /app_clients and in
    // provisionAppClient for every caller (backends/crates/sms-api/src/
    // pair_scope.md), so a tick box for it would always fail.
    expect(KNOWN_SCOPES as readonly string[]).not.toContain("console:pair");
    expect(parseScopes("sms:send console:pair")).toEqual(["sms:send"]);
    expect(serializeScopes(["sms:send"])).not.toContain("console:pair");
  });

  it("offers optout:read, so a key holding it keeps it through an edit, and says it reads every tenant", () => {
    // parseScopes drops what the picker does not know, so a missing entry
    // would silently strip the scope the next time an operator saved the key.
    expect(KNOWN_SCOPES as readonly string[]).toContain("optout:read");
    expect(parseScopes(" sms:send optout:read ")).toEqual(["sms:send", "optout:read"]);
    expect(SCOPE_DESCRIPTIONS["optout:read"]).toBe("Read every opted-out number (all tenants)");
  });
});
