// Pure module (R6): what a failed lookup says. The screen hands over the tRPC
// client error's `data.code` and `message`; this returns either a banner or the
// one field the problem belongs to, in words a support agent can act on.
//
// # It matches on the server's text, and that is a coupling
//
// The gateway answers a missing permission and an exhausted hourly budget with
// the same 403, so only the message tells them apart. The 422s the console can
// place on a field are likewise told apart by their fixed texts. Only some of
// those are pinned by a Rust test: two by equality in `delivery_status.rs`, the
// window width and order by a `contains`, the id and reference ones not at all.
// `delivery_status.md` lists every text this file matches and says that changing
// any of them means changing this file; the permission prefix is `rbac.rs`'s,
// noted in `rbac.md`. A text this table does not know falls back to a banner that
// shows it, so a reworded refusal degrades to less friendly, never to silent.
//
// # The typed number is never repeated
//
// Nothing here quotes input. The only text echoed is the server's own, and the
// server's 422 texts never carry what was typed (`delivery_status.md`). As a
// second line, `describeLookupError` blanks two things out of any server text it
// does echo, so a future message that did quote a number still could not reach
// the screen: the values that were actually sent (`typedValues`), and any run of
// digits shaped like a phone number, whatever was typed.

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

/** A typed value shorter than this is not blanked by itself: a client reference
 * such as `app` or `ord1` is also an ordinary word or a number in a sentence,
 * and the phone-shaped rule below already catches every number. */
const MIN_TYPED_LENGTH = 6;
/** Digits at which a run is read as a phone number (a Cameroonian one has 9). */
const MIN_PHONE_DIGITS = 8;
/** A run of digits with the separators people and servers put in a number
 * (`+237 699 887 766`, `00237699887766`, `(237) 699-887-766`). */
const DIGIT_RUN = /[+(]?\d[\d\s().-]{6,}\d/g;

/** The values the lookup was sent with that a server text could echo. */
export function typedValues(args: Readonly<Record<string, unknown>> | undefined): string[] {
  if (args === undefined) return [];
  return ["msisdn", "clientRef", "messageId", "appId"]
    .map((key) => args[key])
    .filter((value): value is string => typeof value === "string");
}

/** Blanks `text` of what was sent and of anything shaped like a number.
 *
 * Two rules, each for a gap the other leaves:
 * - a sent value of at least `MIN_TYPED_LENGTH` characters is blanked as it
 *   stands (a reference, an id, an app id), whatever it looks like;
 * - a run of at least `MIN_PHONE_DIGITS` digits, with the separators
 *   `DIGIT_RUN` allows, is blanked whatever was typed, so a number typed as
 *   `00237...` and echoed as `+237 ...` (or the reverse) still goes. Eight is
 *   past every figure the gateway's own texts carry (`60`, `90`) and below the
 *   shortest national number; a date is eight digits and would go too, which
 *   costs nothing because no refusal echoes one. */
function redact(text: string, sent: readonly string[]): string {
  // Numbers first: blanking a typed `699887766` first would leave the `+237`
  // of an echoed `+237699887766` behind.
  let out = text.replace(DIGIT_RUN, (run) =>
    run.replace(/\D/g, "").length >= MIN_PHONE_DIGITS ? REDACTED : run,
  );
  for (const raw of sent) {
    const value = raw.trim();
    if (value.length >= MIN_TYPED_LENGTH) out = out.split(value).join(REDACTED);
  }
  return out;
}

/** What to show for a failed lookup. `sent` is what the lookup was sent with
 * (`typedValues`, never the live form, which may have been edited since): used
 * only to keep it out of the text. */
export function describeLookupError(
  error: LookupErrorInput,
  sent: readonly string[] = [],
): LookupErrorView {
  const message = error.message;
  const echo = (text: string) => redact(text, sent);

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
          text: "Your role does not include delivery:read. Support and Auditor hold it by default; a custom role can be granted it. Ask an owner or admin to grant it to your role.",
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
