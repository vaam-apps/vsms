// The one line about this console's key, for the account block: a pure function
// of the status and the clock, so the words are tested rather than assumed.

import { daysLeft, type KeyStatus } from "@/lib/key-lifecycle";

/** "Paired, 12 days left", or what is wrong with the key. */
export function summarizeKey(status: KeyStatus | { kind: "checking" }, now: number): string {
  switch (status.kind) {
    case "paired": {
      const days = daysLeft(status.meta, now);
      if (days === 0) return "Paired, expires today";
      return `Paired, ${days} ${days === 1 ? "day" : "days"} left`;
    }
    case "ended":
      return status.cause === "expired" ? "Key expired" : "Key no longer accepted";
    case "pending":
    case "none":
      return "Not paired";
    case "unreachable":
      return "Gateway unreachable";
    case "clock":
      return "Clock is off";
    case "checking":
      return "Checking";
  }
}
