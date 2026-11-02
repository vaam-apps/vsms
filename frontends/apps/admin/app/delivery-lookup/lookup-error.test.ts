import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { BUDGET_PREFIX, describeLookupError, typedValues } from "./lookup-error";
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
    expect(view.text).toContain("Support and Auditor hold it by default");
    expect(view.text).toContain("a custom role can be granted it");
    // It names who holds it, not who does not: a custom role can be granted it.
    expect(view.text).not.toContain("operator");
    expect(view.text).not.toContain("do not");
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

describe("what was sent is never repeated", () => {
  const forbiddenText = (message: string, sent: readonly string[] = []) =>
    describeLookupError({ code: "FORBIDDEN", message }, sent).text;

  it("typedValues reads the four typed keys of the arguments actually sent", () => {
    expect(
      typedValues({
        msisdn: "+237699887766",
        clientRef: "ORD-7781",
        messageId: "cm9x2kp0a1b3c4d5e6f7g8h",
        appId: "capp9x2kp0a1b3c4d5e6f7",
        since: "2026-10-01T00:00:00.000Z",
        until: "2026-10-08T00:00:00.000Z",
      }),
    ).toEqual(["+237699887766", "ORD-7781", "cm9x2kp0a1b3c4d5e6f7g8h", "capp9x2kp0a1b3c4d5e6f7"]);
    expect(typedValues({ messageId: "cmsg1" })).toEqual(["cmsg1"]);
    expect(typedValues(undefined)).toEqual([]);
  });

  describe("every path that echoes server text blanks it", () => {
    const sent = typedValues({
      msisdn: "+237699887766",
      clientRef: "ORD-7781",
      messageId: "cm9x2kp0a1b3c4d5e6f7g8h",
    });
    const hostile = [
      "msisdn +237699887766 is not valid",
      "the number 237699887766 was refused for ref ORD-7781",
      "bad id cm9x2kp0a1b3c4d5e6f7g8h",
    ];
    it.each(["FORBIDDEN", "BAD_REQUEST"])("%s", (code) => {
      for (const message of hostile) {
        const { text } = describeLookupError({ code, message }, sent);
        for (const secret of [
          "+237699887766",
          "237699887766",
          "ORD-7781",
          "cm9x2kp0a1b3c4d5e6f7g8h",
        ]) {
          expect(text, `${code}: ${message}`).not.toContain(secret);
        }
        expect(text).toContain("[hidden]");
      }
    });
  });

  it("the fixed texts never contain a number", () => {
    const fixed = [
      forbidden(BUDGET),
      forbidden(PERMISSION),
      badRequest("msisdn is not a valid mobile number"),
      describeLookupError({ code: "UNAUTHORIZED", message: "" }),
      describeLookupError({ code: "INTERNAL_SERVER_ERROR", message: "" }),
    ];
    for (const view of fixed) {
      expect(view.text).not.toContain("699887766");
      expect(view.text).not.toContain("[hidden]");
    }
  });

  describe("a number is caught however it was typed and however it is echoed", () => {
    it("typed 00237..., echoed as +237...", () => {
      const sent = typedValues({ msisdn: "00237699887766" });
      expect(forbiddenText("no route for +237699887766", sent)).toBe(
        "The gateway refused this lookup: no route for [hidden]",
      );
    });

    it("typed 237..., echoed as +237 699 887 766", () => {
      const sent = typedValues({ msisdn: "237699887766" });
      expect(forbiddenText("no route for +237 699 887 766 today", sent)).toBe(
        "The gateway refused this lookup: no route for [hidden] today",
      );
    });

    it("typed with spaces or a national 699887766, echoed in either form", () => {
      for (const typed of ["+237 699 887 766", "699887766"]) {
        const sent = typedValues({ msisdn: typed });
        for (const echo of ["+237699887766", "699 887 766", "(237) 699-887-766", "699887766"]) {
          expect(forbiddenText(`x ${echo} y`, sent), `${typed} / ${echo}`).toBe(
            "The gateway refused this lookup: x [hidden] y",
          );
        }
      }
    });

    it("whatever was typed: a number in the text goes with nothing sent at all", () => {
      expect(forbiddenText("x +237699887766 y")).toBe(
        "The gateway refused this lookup: x [hidden] y",
      );
    });
  });

  describe("it is the arguments sent, not the form as it is now", () => {
    it("a number edited after the send is still blanked, and the new value is not what is looked for", () => {
      const sentThen = typedValues({ clientRef: "ORD-7781", appId: "capp9x2kp0a1b3c4d5e6f7" });
      // The form now holds something else; only `sentThen` is passed.
      expect(forbiddenText("unknown ref ORD-7781", sentThen)).toBe(
        "The gateway refused this lookup: unknown ref [hidden]",
      );
      expect(forbiddenText("unknown ref ORD-9999", sentThen)).toBe(
        "The gateway refused this lookup: unknown ref ORD-9999",
      );
    });
  });

  describe("it does not eat ordinary text", () => {
    it("a short client reference such as `app` blanks nothing", () => {
      const sent = typedValues({ clientRef: "app", appId: "x1" });
      expect(forbiddenText("an app key looks up its own app's messages only", sent)).toBe(
        "The gateway refused this lookup: an app key looks up its own app's messages only",
      );
    });

    it("a typed value of exactly six characters is blanked, five is not", () => {
      expect(forbiddenText("ref ABC123 and ABC12", typedValues({ clientRef: "ABC123" }))).toBe(
        "The gateway refused this lookup: ref [hidden] and ABC12",
      );
      expect(forbiddenText("ref ABC12", typedValues({ clientRef: "ABC12" }))).toBe(
        "The gateway refused this lookup: ref ABC12",
      );
    });

    it("the gateway's own figures survive: 60 an hour, 90 days, 1 to 50", () => {
      const text = "60 lookups per hour, 90 days wide, limit 1 to 50, 2 or 3 more";
      expect(forbiddenText(text)).toBe(`The gateway refused this lookup: ${text}`);
    });

    it("seven digits are left alone, eight go", () => {
      expect(forbiddenText("code 1234567 here")).toContain("1234567");
      expect(forbiddenText("code 12345678 here")).toContain("[hidden]");
    });
  });

  it("without the sent values, an id would pass through: the argument is what protects it", () => {
    expect(forbiddenText("x cm9x2kp0a1b3c4d5e6f7g8h y")).toContain("cm9x2kp0a1b3c4d5e6f7g8h");
  });

  it("leaves empty and short values alone", () => {
    expect(forbiddenText("a b c", ["", " ", "a"])).toBe("The gateway refused this lookup: a b c");
  });

  it("the screen asks about the arguments the mutation carries, never the live form", () => {
    const screen = readFileSync(
      fileURLToPath(new URL("delivery-lookup-screen.tsx", import.meta.url)),
      "utf8",
    );
    const calls = [...screen.matchAll(/typedValues\(([^)]*)\)/g)].map((m) => (m[1] ?? "").trim());
    expect(calls.sort()).toEqual(["lookup.variables", "sent"]);
  });
});
