// Pure module (R6): the Delivery lookup's form — which key was chosen, what a
// valid answer to it looks like, the window a lookup by client reference or by
// number asks about, and the arguments `deliveryStatus.lookup` is sent. No
// React, no clock of its own: every function that needs "today" takes it, so a
// test pins it.
//
// # The window is always explicit
//
// A lookup by client reference or by number is bounded in time. The server
// would default a missing window to the last 7 days, but the page sends the
// `since` and `until` it shows, so what it says it asked is exactly what it
// asked. Dates are UTC, as everywhere in the console (`messages/date-range.ts`):
// `since` is the start of the From day, `until` the start of the day after the
// To day, so both days are whole.
//
// # The server's 90-day rules, and why the earliest From is today minus 89
//
// The window may not be wider than 90 days, and a lookup by number may not
// start more than 90 days before now (`backends/crates/sms-api/src/
// delivery_status.rs`). "Today minus 90" at midnight is already before "now
// minus 90 days" at any other hour, so the earliest From that is always
// accepted is today minus 89, which also makes From..To (inclusive) exactly 90
// days, the widest the server allows.
//
// # What this module never does
//
// It validates the shape of what was typed and never echoes it: no message
// here quotes the number, the reference or an id.

import { z } from "zod";
import { isValidMessageId } from "../messages/[id]/message-id";

/** Which key a lookup names. Exactly one per lookup; the URL's `by`. */
export const LOOKUP_KINDS = ["id", "ref", "number"] as const;
export type LookupKind = (typeof LOOKUP_KINDS)[number];
export const DEFAULT_KIND: LookupKind = "id";

/** The server's cap on a window's width, and on how far back a number reaches. */
export const MAX_WINDOW_DAYS = 90;

/** The one-tap windows, by the number of days they cover, today included. */
export const WINDOW_PRESETS = [
  { days: 7, label: "Last 7 days" },
  { days: 30, label: "Last 30 days" },
  { days: 90, label: "Last 90 days" },
] as const;
export const DEFAULT_PRESET_DAYS = 7;

/** Field messages shared with `lookup-error.ts`, which shows the same words when
 * the server is the one that refused. */
export const FIELD_MESSAGES = {
  numberReach: "A lookup by phone number reaches back 90 days at most.",
  windowWidth: "Choose a window of 90 days or less.",
  windowOrder: "The end date is before the start date.",
  numberWhole:
    "Enter the whole mobile number, e.g. +237677123456. A partial number is never matched.",
} as const;

export interface LookupFormValues {
  messageId: string;
  appId: string;
  clientRef: string;
  msisdn: string;
  /** UTC dates, `YYYY-MM-DD`; empty when cleared. Unused by a lookup by id. */
  from: string;
  to: string;
}

const DAY_MS = 24 * 60 * 60 * 1000;

/** A UTC calendar date, `YYYY-MM-DD`, `offsetDays` after (negative: before) `now`'s. */
export function utcDate(now: Date, offsetDays = 0): string {
  return new Date(now.getTime() + offsetDays * DAY_MS).toISOString().slice(0, 10);
}

/** The From..To a preset of `days` selects: ends today, covers `days` UTC days. */
export function presetWindow(days: number, now: Date): { from: string; to: string } {
  return { from: utcDate(now, -(days - 1)), to: utcDate(now) };
}

/** How many days a preset covers if the current window is exactly one, so its
 * chip shows selected and stops the moment either date is edited by hand. */
export function activePresetDays(from: string, to: string, now: Date): number | null {
  for (const { days } of WINDOW_PRESETS) {
    const window = presetWindow(days, now);
    if (window.from === from && window.to === to) return days;
  }
  return null;
}

/** The earliest From a lookup by `kind` accepts, or `undefined` for no floor. */
export function earliestFrom(kind: LookupKind, now: Date): string | undefined {
  return kind === "number" ? utcDate(now, -(MAX_WINDOW_DAYS - 1)) : undefined;
}

export function defaultValues(now: Date): LookupFormValues {
  return {
    messageId: "",
    appId: "",
    clientRef: "",
    msisdn: "",
    ...presetWindow(DEFAULT_PRESET_DAYS, now),
  };
}

/** Whole UTC days from `from` to `to`, both included; `NaN` for a date that is
 * not one. */
function inclusiveDays(from: string, to: string): number {
  const start = Date.parse(`${from}T00:00:00.000Z`);
  const end = Date.parse(`${to}T00:00:00.000Z`);
  return (end - start) / DAY_MS + 1;
}

/** The form's schema for one kind of lookup. Only the fields that kind uses are
 * checked, so a value left in another kind's field never blocks a submit. */
export function lookupSchema(kind: LookupKind, now: Date) {
  const floor = earliestFrom(kind, now);
  return z
    .object({
      messageId: z.string(),
      appId: z.string(),
      clientRef: z.string(),
      msisdn: z.string(),
      from: z.string(),
      to: z.string(),
    })
    .superRefine((values, ctx) => {
      const fail = (path: keyof LookupFormValues, message: string) =>
        ctx.addIssue({ code: "custom", path: [path], message });

      if (kind === "id") {
        const id = values.messageId.trim();
        if (id === "") fail("messageId", "Enter the message id.");
        else if (!isValidMessageId(id))
          fail("messageId", "That is not a message id: ids are lowercase letters and digits.");
        return;
      }

      if (kind === "ref") {
        if (values.clientRef.trim() === "") fail("clientRef", "Enter the client reference.");
        const appId = values.appId.trim();
        if (appId === "") fail("appId", "Name the app this reference belongs to.");
        else if (!isValidMessageId(appId))
          fail("appId", "That is not an app id: ids are lowercase letters and digits.");
      } else if (values.msisdn.trim() === "") {
        fail("msisdn", "Enter the whole mobile number.");
      }

      if (values.from === "") fail("from", "Choose a start date.");
      if (values.to === "") fail("to", "Choose an end date.");
      if (values.from === "" || values.to === "") return;

      if (values.from > values.to) fail("to", FIELD_MESSAGES.windowOrder);
      else if (inclusiveDays(values.from, values.to) > MAX_WINDOW_DAYS)
        fail("from", FIELD_MESSAGES.windowWidth);
      else if (floor !== undefined && values.from < floor) fail("from", FIELD_MESSAGES.numberReach);
    });
}

/** The start of the day after `date`, as an ISO instant. */
function startOfNextDay(date: string): string {
  return new Date(Date.parse(`${date}T00:00:00.000Z`) + DAY_MS).toISOString();
}

/** The arguments of `deliveryStatus.lookup`. */
export interface LookupArgs {
  messageId?: string;
  appId?: string;
  clientRef?: string;
  msisdn?: string;
  since?: string;
  until?: string;
}

/** What `deliveryStatus.lookup` is sent: exactly one key, the window only where
 * the kind has one, no key left empty. Assumes `values` passed `lookupSchema`. */
export function toLookupArgs(values: LookupFormValues, kind: LookupKind): LookupArgs {
  if (kind === "id") return { messageId: values.messageId.trim() };
  const window = {
    since: `${values.from}T00:00:00.000Z`,
    until: startOfNextDay(values.to),
  };
  if (kind === "ref") {
    return { appId: values.appId.trim(), clientRef: values.clientRef.trim(), ...window };
  }
  return { msisdn: values.msisdn.trim(), ...window };
}

/** Reads the URL's `by` back into a kind; anything else is the default. */
export function parseKind(value: string | null | undefined): LookupKind {
  return (LOOKUP_KINDS as readonly string[]).includes(value ?? "")
    ? (value as LookupKind)
    : DEFAULT_KIND;
}
