import { describe, expect, it } from "vitest";
import {
  activePresetDays,
  defaultValues,
  earliestFrom,
  FIELD_MESSAGES,
  type LookupFormValues,
  lookupSchema,
  MAX_WINDOW_DAYS,
  parseKind,
  presetWindow,
  toLookupArgs,
  utcDate,
  WINDOW_PRESETS,
} from "./lookup-query";

const DAY_MS = 24 * 60 * 60 * 1000;
// Late in a UTC day on purpose: the boundary that matters is "today minus 90
// days at midnight" against "now minus 90 days", which only differ after 00:00.
const NOW = new Date("2026-10-30T23:59:00.000Z");
const ID = "cmsg0000000000000000001";
const APP = "capp0000000000000000001";

function values(over: Partial<LookupFormValues> = {}): LookupFormValues {
  return { ...defaultValues(NOW), ...over };
}

function problems(kind: Parameters<typeof lookupSchema>[0], over: Partial<LookupFormValues>) {
  const result = lookupSchema(kind, NOW).safeParse(values(over));
  if (result.success) return {};
  return Object.fromEntries(result.error.issues.map((i) => [String(i.path[0]), i.message]));
}

describe("the window", () => {
  it("a preset of N days ends today and covers N UTC days", () => {
    expect(presetWindow(7, NOW)).toEqual({ from: "2026-10-24", to: "2026-10-30" });
    expect(presetWindow(90, NOW)).toEqual({ from: "2026-08-02", to: "2026-10-30" });
  });

  it("the default is the last 7 days", () => {
    expect(defaultValues(NOW)).toMatchObject({ from: "2026-10-24", to: "2026-10-30" });
    expect(activePresetDays("2026-10-24", "2026-10-30", NOW)).toBe(7);
  });

  it("a preset stops being active once a date is edited by hand", () => {
    expect(activePresetDays("2026-10-25", "2026-10-30", NOW)).toBeNull();
    expect(activePresetDays("2026-10-24", "2026-10-29", NOW)).toBeNull();
  });

  it("a number cannot start before today minus 89 days; the other kinds have no floor", () => {
    expect(earliestFrom("number", NOW)).toBe("2026-08-02");
    expect(earliestFrom("ref", NOW)).toBeUndefined();
    expect(earliestFrom("id", NOW)).toBeUndefined();
  });

  it("every preset is a window the server accepts for a number, at any hour of the day", () => {
    for (const hour of [0, 1, 12, 23]) {
      const now = new Date(`2026-10-30T${String(hour).padStart(2, "0")}:30:00.000Z`);
      for (const { days } of WINDOW_PRESETS) {
        const args = toLookupArgs(
          { ...defaultValues(now), msisdn: "677123456", ...presetWindow(days, now) },
          "number",
        );
        const since = Date.parse(String(args.since));
        const until = Date.parse(String(args.until));
        expect(since, `${days}d at ${hour}h reaches back too far`).toBeGreaterThanOrEqual(
          now.getTime() - MAX_WINDOW_DAYS * DAY_MS,
        );
        expect(until - since, `${days}d at ${hour}h is too wide`).toBeLessThanOrEqual(
          MAX_WINDOW_DAYS * DAY_MS,
        );
        expect(
          lookupSchema("number", now).safeParse({
            ...defaultValues(now),
            msisdn: "677123456",
            ...presetWindow(days, now),
          }).success,
        ).toBe(true);
      }
    }
  });

  it("today minus 90 days is refused for a number: it is before now minus 90 days", () => {
    const early = { msisdn: "677123456", from: utcDate(NOW, -90), to: utcDate(NOW, -80) };
    expect(problems("number", early)).toEqual({ from: FIELD_MESSAGES.numberReach });
    // The same days are fine for a reference, which has no floor.
    expect(problems("ref", { ...early, clientRef: "ORD-1", appId: APP })).toEqual({});
  });

  it("refuses From after To, on the To field", () => {
    expect(
      problems("ref", { clientRef: "ORD-1", appId: APP, from: "2026-10-30", to: "2026-10-29" }),
    ).toEqual({
      to: FIELD_MESSAGES.windowOrder,
    });
  });

  it("refuses a window wider than 90 days, on the From field", () => {
    expect(
      problems("ref", { clientRef: "ORD-1", appId: APP, from: "2026-07-01", to: "2026-10-30" }),
    ).toEqual({
      from: FIELD_MESSAGES.windowWidth,
    });
  });

  it("accepts exactly 90 days for a reference", () => {
    expect(
      problems("ref", { clientRef: "ORD-1", appId: APP, from: "2026-08-02", to: "2026-10-30" }),
    ).toEqual({});
  });

  it("asks for both dates when one is cleared", () => {
    expect(problems("ref", { clientRef: "ORD-1", appId: APP, from: "" })).toEqual({
      from: "Choose a start date.",
    });
    expect(problems("number", { msisdn: "677123456", to: "" })).toEqual({
      to: "Choose an end date.",
    });
  });
});

describe("one key per lookup", () => {
  it("an id is sent alone, trimmed, with no window", () => {
    expect(
      toLookupArgs(
        values({ messageId: ` ${ID} `, clientRef: "stale", msisdn: "+237677123456" }),
        "id",
      ),
    ).toEqual({
      messageId: ID,
    });
  });

  it("a reference is sent with its app and an explicit window, and trimmed", () => {
    expect(
      toLookupArgs(
        values({ clientRef: "  ORD-1  ", appId: ` ${APP} `, from: "2026-10-24", to: "2026-10-30" }),
        "ref",
      ),
    ).toEqual({
      appId: APP,
      clientRef: "ORD-1",
      since: "2026-10-24T00:00:00.000Z",
      until: "2026-10-31T00:00:00.000Z",
    });
  });

  it("a number is sent alone with an explicit window", () => {
    const args = toLookupArgs(
      values({ msisdn: " +237677123456 ", messageId: ID, clientRef: "x", appId: APP }),
      "number",
    );
    expect(args).toEqual({
      msisdn: "+237677123456",
      since: "2026-10-24T00:00:00.000Z",
      until: "2026-10-31T00:00:00.000Z",
    });
    expect(Object.keys(args).sort()).toEqual(["msisdn", "since", "until"]);
  });

  it("the end of the window rolls over a month and a year", () => {
    expect(
      toLookupArgs(values({ msisdn: "1", from: "2026-12-30", to: "2026-12-31" }), "number").until,
    ).toBe("2027-01-01T00:00:00.000Z");
  });
});

describe("what each kind requires", () => {
  it("an id must look like one", () => {
    expect(problems("id", { messageId: "" })).toEqual({ messageId: "Enter the message id." });
    expect(problems("id", { messageId: "Not An Id!" })).toEqual({
      messageId: "That is not a message id: ids are lowercase letters and digits.",
    });
    expect(problems("id", { messageId: ID })).toEqual({});
  });

  it("an id needs no window and ignores a bad one", () => {
    expect(problems("id", { messageId: ID, from: "", to: "" })).toEqual({});
  });

  it("a reference needs the reference and an app that looks like an id", () => {
    expect(problems("ref", { clientRef: "   ", appId: "" })).toEqual({
      clientRef: "Enter the client reference.",
      appId: "Name the app this reference belongs to.",
    });
    expect(problems("ref", { clientRef: "ORD-1", appId: "Mobile Money" })).toEqual({
      appId: "That is not an app id: ids are lowercase letters and digits.",
    });
  });

  it("a number only has to be present: the server decides if it is a mobile number", () => {
    expect(problems("number", { msisdn: "  " })).toEqual({
      msisdn: "Enter the whole mobile number.",
    });
    expect(problems("number", { msisdn: "12" })).toEqual({});
  });

  it("another kind's leftover field never blocks a submit", () => {
    expect(
      problems("number", { msisdn: "677123456", messageId: "!!", clientRef: "", appId: "bad app" }),
    ).toEqual({});
  });
});

describe("the URL's kind", () => {
  it("reads id, ref and number and falls back to id", () => {
    expect(parseKind("ref")).toBe("ref");
    expect(parseKind("number")).toBe("number");
    expect(parseKind("msisdn")).toBe("id");
    expect(parseKind(null)).toBe("id");
  });
});
