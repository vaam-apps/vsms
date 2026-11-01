import { describe, expect, it } from "vitest";
import { BUDGET_PREFIX, describeLookupError, type LookupErrorView } from "./lookup-error";
import { FIELD_MESSAGES } from "./lookup-query";

// The server's texts, copied from `backends/crates/sms-api/src/delivery_status.rs`
// and `rbac.rs`. If one of those changes, this table fails on purpose.
const BUDGET = "delivery lookup budget exhausted: 60 lookups an hour";
const PERMISSION = 'missing required permission "delivery:read"';

const forbidden = (message: string) => describeLookupError({ code: "FORBIDDEN", message });
const badRequest = (message: string) => describeLookupError({ code: "BAD_REQUEST", message });

describe("a 403 is told apart by its text", () => {
  it("an exhausted budget says the hour's 60 are used and that ids still work", () => {
    const view = forbidden(BUDGET);
    expect(view.kind).toBe("banner");
    expect(view.text).toBe(
      "You have used this hour's 60 lookups by phone number or client reference. Lookups by message id still work; the others reopen as the hour rolls on.",
    );
  });

  it("a missing permission names the role rule and who to ask", () => {
    const view = forbidden(PERMISSION);
    expect(view.kind).toBe("banner");
    expect(view.text).toContain("delivery:read");
    expect(view.text).toContain("Support and Auditor hold it");
    expect(view.text).toContain("Ask an owner or admin");
  });

  it("the two are not confused: neither text produces the other's banner", () => {
    expect(forbidden(BUDGET).text).not.toBe(forbidden(PERMISSION).text);
    expect(forbidden(`${BUDGET_PREFIX}, whatever follows`).text).toBe(forbidden(BUDGET).text);
  });

  it("any other 403 shows the gateway's own words", () => {
    expect(forbidden("an app key looks up its own app's messages only")).toEqual({
      kind: "banner",
      text: "The gateway refused this lookup: an app key looks up its own app's messages only",
    });
  });
});

describe("a 422 the page can place on a field", () => {
  const cases: [string, string, string][] = [
    ["msisdn is not a valid mobile number", "msisdn", FIELD_MESSAGES.numberWhole],
    ["a lookup by number may not start more than 90 days ago", "from", FIELD_MESSAGES.numberReach],
    ["the window may not be wider than 90 days", "from", FIELD_MESSAGES.windowWidth],
    ["since is after until", "to", FIELD_MESSAGES.windowOrder],
    ["messageId is not an id", "messageId", "messageId is not an id"],
    ["appId is not an id", "appId", "appId is not an id"],
    ["clientRef must not be blank", "clientRef", "clientRef must not be blank"],
    [
      "a clientRef belongs to one app: name it with appId",
      "appId",
      "a clientRef belongs to one app: name it with appId",
    ],
  ];

  it.each(cases)("%s", (message, field, text) => {
    expect(badRequest(message)).toEqual({ kind: "field", field, text });
  });

  it("any other 422 is a banner with the server's text", () => {
    expect(badRequest("name exactly one of messageId, clientRef or msisdn")).toEqual({
      kind: "banner",
      text: "name exactly one of messageId, clientRef or msisdn",
    });
  });

  it("a text that is only close to a known one is not placed on a field", () => {
    expect(badRequest("msisdn is not a valid mobile number.").kind).toBe("banner");
  });
});

describe("the other failures", () => {
  it("UNAUTHORIZED asks for the console to be paired again", () => {
    expect(describeLookupError({ code: "UNAUTHORIZED", message: "x" })).toEqual({
      kind: "banner",
      text: "This console's key is not accepted any more. Pair it again.",
    });
  });

  it("a server error or a network failure says nothing is shown and the budget may be spent", () => {
    for (const code of ["INTERNAL_SERVER_ERROR", "TIMEOUT", undefined]) {
      const view = describeLookupError({ code, message: "fetch failed" });
      expect(view.kind).toBe("banner");
      expect(view.text).toContain("nothing is shown rather than a partial answer");
      expect(view.text).toContain("may still have counted against this hour's budget");
      expect(view.text).not.toContain("fetch failed");
    }
  });
});

describe("what the person typed is never repeated", () => {
  const NUMBER = "+237699887766";
  const TYPED = [NUMBER, "ORD-7781", "cmsg0000000000000000001"];

  // The server never quotes input, but if a message ever did, every path that
  // echoes server text must still blank it.
  const hostile = [
    `msisdn ${NUMBER} is not valid`,
    `${NUMBER} budget`,
    `the number 237699887766 was refused for ref ORD-7781`,
    `bad id cmsg0000000000000000001`,
  ];

  function allText(view: LookupErrorView): string {
    return view.text;
  }

  it("blanks the number out of every echoed server text", () => {
    for (const code of [
      "FORBIDDEN",
      "BAD_REQUEST",
      "UNAUTHORIZED",
      "INTERNAL_SERVER_ERROR",
      undefined,
    ]) {
      for (const message of hostile) {
        const text = allText(describeLookupError({ code, message }, TYPED));
        for (const secret of [NUMBER, "237699887766", "ORD-7781", "cmsg0000000000000000001"]) {
          expect(text, `${code}: ${message}`).not.toContain(secret);
        }
      }
    }
  });

  it("the fixed texts never contain the number that was typed", () => {
    const fixed = [
      forbidden(BUDGET),
      forbidden(PERMISSION),
      badRequest("msisdn is not a valid mobile number"),
      describeLookupError({ code: "UNAUTHORIZED", message: "" }),
      describeLookupError({ code: "INTERNAL_SERVER_ERROR", message: "" }),
    ];
    for (const view of fixed) {
      for (const secret of [NUMBER, "237699887766", "ORD-7781"]) {
        expect(view.text).not.toContain(secret);
      }
    }
  });

  it("without the typed values, a quoted number would pass through: the argument is what protects", () => {
    const text = describeLookupError({ code: "FORBIDDEN", message: `x ${NUMBER} y` }).text;
    expect(text).toContain(NUMBER);
  });

  it("leaves short values alone, so an empty field blanks nothing", () => {
    const text = describeLookupError({ code: "FORBIDDEN", message: "a b c" }, ["", " ", "a"]).text;
    expect(text).toBe("The gateway refused this lookup: a b c");
  });
});
