// #221: the seam every `@vsms/gateway` module now shares instead of its own
// `parseJsonBody` + local `normalize*` pair. See `json.ts`'s own module doc
// for the full reasoning — this file proves the specific claims that doc
// makes: a top-level null, a nested null, a null inside an array, the
// paged envelope, and — the actual hazard the issue asked to be checked
// before writing any of this — that a verbatim JSON-as-string field (like
// `WebhookAttempt.payload`) survives completely untouched, including when
// its own encoded text contains the literal substring `null`.

import { describe, expect, it } from "vitest";
import { normalizeGatewayJson, parseGatewayJson } from "./json";

function textResponse(text: string): { text(): Promise<string> } {
  return { text: () => Promise.resolve(text) };
}

describe("normalizeGatewayJson", () => {
  it("converts a top-level-object field's null to undefined", () => {
    const result = normalizeGatewayJson({ id: "a", description: null });
    expect(result).toEqual({ id: "a", description: undefined });
    expect(result.description).toBeUndefined();
  });

  it("converts a null nested one level down (a detail row inside an envelope)", () => {
    const result = normalizeGatewayJson({
      items: [{ id: "a", healthCheckedAt: null }],
      totalCount: 1,
    });
    const [item] = result.items;
    expect(result.items).toHaveLength(1);
    expect(item?.healthCheckedAt).toBeUndefined();
  });

  it("converts a null nested two levels down (a route evaluation inside a simulate-route result)", () => {
    const result = normalizeGatewayJson({
      evaluations: [{ routeId: "r1", predicateKind: null, predicateExpected: null }],
      tieBreak: null,
      winner: { routeId: "r1", failoverRouteId: null },
    });
    const [evaluation] = result.evaluations;
    expect(result.evaluations).toHaveLength(1);
    expect(evaluation?.predicateKind).toBeUndefined();
    expect(evaluation?.predicateExpected).toBeUndefined();
    expect(result.tieBreak).toBeUndefined();
    expect(result.winner.failoverRouteId).toBeUndefined();
  });

  it("converts null inside an array of plain values, not just objects", () => {
    const result = normalizeGatewayJson([1, null, "x", null]);
    expect(result).toEqual([1, undefined, "x", undefined]);
  });

  it("converts a bare top-level null", () => {
    expect(normalizeGatewayJson(null)).toBeUndefined();
  });

  it("leaves non-null primitives, zero, false, and empty string untouched", () => {
    const result = normalizeGatewayJson({ count: 0, active: false, notes: "" });
    expect(result).toEqual({ count: 0, active: false, notes: "" });
  });

  // --- The paged envelope every @@paged model returns. --------------------
  it("normalizes every item inside a { items, totalCount, pageInfo } envelope", () => {
    const result = normalizeGatewayJson({
      items: [
        { id: "m1", clientRef: null, submittedAt: "2026-08-08T14:03:08.312Z" },
        { id: "m2", clientRef: "ref-2", submittedAt: null },
      ],
      totalCount: 2,
      pageInfo: { limit: 50, offset: null, hasNextPage: false, hasPreviousPage: false },
    });
    const [first, second] = result.items;
    expect(result.items).toHaveLength(2);
    expect(first?.clientRef).toBeUndefined();
    expect(first?.submittedAt).toBe("2026-08-08T14:03:08.312Z");
    expect(second?.clientRef).toBe("ref-2");
    expect(second?.submittedAt).toBeUndefined();
    // Never read by any caller (json.ts's own module doc) — normalizing it
    // is harmless, asserted here so a future reader doesn't have to
    // rediscover that by grepping.
    expect(result.pageInfo.offset).toBeUndefined();
  });

  // --- The hazard the task exists to guard against. ------------------------
  describe("verbatim JSON-as-string fields are never descended into", () => {
    it("leaves WebhookAttempt.payload's encoded text byte-for-byte untouched, literal 'null' included", () => {
      const encoded = '{"messageId":"m1","stateReason":null,"clientRef":null}';
      const result = normalizeGatewayJson({
        id: "att1",
        eventType: "message.failed",
        payload: encoded,
        lastError: null,
      });
      // The string itself — including the literal substring "null" inside
      // it twice — must be returned exactly as received: not re-parsed,
      // not re-serialised, not touched at all.
      expect(result.payload).toBe(encoded);
      expect(typeof result.payload).toBe("string");
      // The sibling field, a genuine absent-column null, still normalizes.
      expect(result.lastError).toBeUndefined();
    });

    it("leaves the audit log's actor/before/after/primaryKey snapshots untouched", () => {
      const actor = '{"sub":"u1","kind":"user","role":"owner"}';
      const before = '{"active":true,"description":null}';
      const after = '{"active":false,"description":null}';
      const primaryKey = '{"id":"a1"}';
      const result = normalizeGatewayJson({
        eventId: "e1",
        model: "App",
        operation: "update",
        actor,
        before,
        after,
        primaryKey,
        tenant: null,
      });
      expect(result.actor).toBe(actor);
      expect(result.before).toBe(before);
      expect(result.after).toBe(after);
      expect(result.primaryKey).toBe(primaryKey);
      expect(result.tenant).toBeUndefined();
    });

    it("leaves Provider.config untouched even though it JSON-encodes nulls internally", () => {
      const config = '{"apiKey":"env:X","timeout":null}';
      const result = normalizeGatewayJson({ id: "p1", config, healthCheckedAt: null });
      expect(result.config).toBe(config);
      expect(result.healthCheckedAt).toBeUndefined();
    });
  });
});

describe("parseGatewayJson", () => {
  it("parses and normalizes a real response body", async () => {
    const result = await parseGatewayJson(
      textResponse(JSON.stringify({ id: "r1", matchOperator: null, matchClass: "otp" })),
    );
    expect(result).toEqual({ id: "r1", matchOperator: undefined, matchClass: "otp" });
  });

  it("returns undefined for an empty body without attempting to parse it", async () => {
    expect(await parseGatewayJson(textResponse(""))).toBeUndefined();
  });

  it("wraps unparseable text as an UNPARSEABLE_RESPONSE object, matching every prior local copy", async () => {
    const result = await parseGatewayJson(textResponse("not json"));
    expect(result).toEqual({ code: "UNPARSEABLE_RESPONSE", message: "not json" });
  });
});
