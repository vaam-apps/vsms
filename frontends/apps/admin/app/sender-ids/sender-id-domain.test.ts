import { describe, expect, it } from "vitest";
import type { ProviderListItem, RegistrationListItem } from "./sender-id-domain";
import { summarizeRegistrations } from "./sender-id-domain";

// Only `providerId`/`status` are read by `summarizeRegistrations` — these
// fixtures carry the minimum shape it actually touches, not every field the
// real tRPC router output has.
function registration(providerId: string, status: string): RegistrationListItem {
  return { providerId, status } as unknown as RegistrationListItem;
}

function providerMap(entries: Array<[id: string, key: string]>): Map<string, ProviderListItem> {
  const map = new Map<string, ProviderListItem>();
  for (const [id, key] of entries) {
    map.set(id, { id, key } as unknown as ProviderListItem);
  }
  return map;
}

describe("summarizeRegistrations", () => {
  it("reports 'not registered anywhere' for an empty list", () => {
    expect(summarizeRegistrations([], new Map())).toBe("not registered anywhere");
  });

  it("joins provider key and status for each registration", () => {
    const providers = providerMap([
      ["p1", "orange_cm"],
      ["p2", "mtn_aggregator"],
    ]);
    const result = summarizeRegistrations(
      [registration("p1", "approved"), registration("p2", "pending")],
      providers,
    );
    expect(result).toBe("orange_cm: approved · mtn_aggregator: pending");
  });

  it("falls back to the raw provider id when the provider isn't in the map", () => {
    const result = summarizeRegistrations(
      [registration("unknown-provider", "rejected")],
      new Map(),
    );
    expect(result).toBe("unknown-provider: rejected");
  });
});
