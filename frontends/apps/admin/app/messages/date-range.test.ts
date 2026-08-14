import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { daysAgoIsoDate, nextDayIso, todayIsoDate } from "./date-range";

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date("2026-08-14T09:30:00.000Z"));
});

afterEach(() => {
  vi.useRealTimers();
});

describe("todayIsoDate", () => {
  it("returns the current UTC date, date-only", () => {
    expect(todayIsoDate()).toBe("2026-08-14");
  });
});

describe("daysAgoIsoDate", () => {
  it("steps back the given number of UTC days", () => {
    expect(daysAgoIsoDate(7)).toBe("2026-08-07");
    expect(daysAgoIsoDate(30)).toBe("2026-07-15");
  });

  it("crosses a UTC month boundary correctly", () => {
    vi.setSystemTime(new Date("2026-08-02T00:00:00.000Z"));
    expect(daysAgoIsoDate(5)).toBe("2026-07-28");
  });

  it("returns today for zero days ago", () => {
    expect(daysAgoIsoDate(0)).toBe(todayIsoDate());
  });
});

describe("nextDayIso", () => {
  it("steps a date-only string one day forward, as a full ISO timestamp at midnight UTC", () => {
    expect(nextDayIso("2026-08-08")).toBe("2026-08-09T00:00:00.000Z");
  });

  it("crosses a UTC month boundary correctly", () => {
    expect(nextDayIso("2026-07-31")).toBe("2026-08-01T00:00:00.000Z");
  });

  it("crosses a UTC year boundary correctly", () => {
    expect(nextDayIso("2026-12-31")).toBe("2027-01-01T00:00:00.000Z");
  });
});
