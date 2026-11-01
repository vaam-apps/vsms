// Pure module (R6): what a failed lookup says. The screen hands over the tRPC
// client error's `data.code` and `message`; this returns either a banner or the
// one field the problem belongs to, in words a support agent can act on.
//
// # It matches on the server's text, and that is a coupling
//
// The gateway answers a missing permission and an exhausted hourly budget with
// the same 403, so only the message tells them apart. The 422s the console can
// place on a field are likewise told apart by their fixed texts. They are pinned
// by Rust unit tests in `backends/crates/sms-api/src/delivery_status.rs`, and the
// budget text by its prefix; `delivery_status.md` records that changing any of
// them means changing this file. A text this table does not know falls back to a
// banner that shows it, so a reworded refusal degrades to less friendly, never
// to silent.
//
// # The typed number is never repeated
//
// Nothing here quotes input. The only text echoed is the server's own, and the
// server's 422 texts never carry what was typed (`delivery_status.md`). As a
// second line, `describeLookupError` also takes the values the person typed and
// blanks them out of any server text it does echo, so a future message that did
// quote a number still could not reach the screen.

import { FIELD_MESSAGES, type LookupFormValues, MAX_WINDOW_DAYS } from "./lookup-query";

/** The slice of a tRPC client error this needs. */
export interface LookupErrorInput {
  code: string | undefined;
  message: string;
}

export type LookupErrorView =
  | { kind: "banner"; text: string }
  | { kind: "field"; field: keyof LookupFormValues; text: string };

/** The 403 a spent hourly budget answers with (a prefix; the rest is the server's). */
export const BUDGET_PREFIX = "delivery lookup budget exhausted";
/** The 403 a role without `delivery:read` answers with (a prefix). */
export const PERMISSION_PREFIX = "missing required permission";

/** The fixed 422 texts that belong to one field, keyed by the server's exact
 * words. `field` is where the message is shown; `text` is what it says, or
 * `undefined` to show the server's own. */
const FIELD_REFUSALS: ReadonlyMap<
  string,
  { field: keyof LookupFormValues; text: string | undefined }
> = new Map([
  ["msisdn is not a valid mobile number", { field: "msisdn", text: FIELD_MESSAGES.numberWhole }],
  [
    `a lookup by number may not start more than ${MAX_WINDOW_DAYS} days ago`,
    { field: "from", text: FIELD_MESSAGES.numberReach },
  ],
  [
    `the window may not be wider than ${MAX_WINDOW_DAYS} days`,
    { field: "from", text: FIELD_MESSAGES.windowWidth },
  ],
  ["since is after until", { field: "to", text: FIELD_MESSAGES.windowOrder }],
  ["messageId is not an id", { field: "messageId", text: undefined }],
  ["appId is not an id", { field: "appId", text: undefined }],
  ["clientRef must not be blank", { field: "clientRef", text: undefined }],
  ["a clientRef belongs to one app: name it with appId", { field: "appId", text: undefined }],
]);

const REDACTED = "[hidden]";

/** Blanks every typed value out of `text`: the value as typed (trimmed) and,
 * when it holds enough digits to be a number, its digits alone, so a server
 * that reformatted a number still could not repeat it. */
function redact(text: string, typed: readonly string[]): string {
  let out = text;
  for (const raw of typed) {
    const value = raw.trim();
    if (value.length < 3) continue;
    out = out.split(value).join(REDACTED);
    const digits = value.replace(/\D/g, "");
    if (digits.length >= 6) out = out.split(digits).join(REDACTED);
  }
  return out;
}

/** What to show for a failed lookup. `typed` is what the person entered
 * (number, reference, ids): used only to keep it out of the text. */
export function describeLookupError(
  error: LookupErrorInput,
  typed: readonly string[] = [],
): LookupErrorView {
  const message = error.message;
  const echo = (text: string) => redact(text, typed);

  switch (error.code) {
    case "FORBIDDEN": {
      if (message.startsWith(BUDGET_PREFIX)) {
        return {
          kind: "banner",
          text: "You have used this hour's 60 lookups by phone number or client reference. Lookups by message id still work; the others reopen as the hour rolls on.",
        };
      }
      if (message.startsWith(PERMISSION_PREFIX)) {
        return {
          kind: "banner",
          text: "Your role does not include delivery:read. Support and Auditor hold it; owner, admin and operator do not. Ask an owner or admin to grant it to your role.",
        };
      }
      return { kind: "banner", text: `The gateway refused this lookup: ${echo(message)}` };
    }
    case "BAD_REQUEST": {
      const refusal = FIELD_REFUSALS.get(message);
      if (refusal !== undefined) {
        return { kind: "field", field: refusal.field, text: refusal.text ?? message };
      }
      return { kind: "banner", text: echo(message) };
    }
    case "UNAUTHORIZED":
      return {
        kind: "banner",
        text: "This console's key is not accepted any more. Pair it again.",
      };
    default:
      return {
        kind: "banner",
        text: "The lookup did not complete, and nothing is shown rather than a partial answer. Try again. A lookup by number or client reference may still have counted against this hour's budget.",
      };
  }
}
