import { PhoneDisplay } from "@vaam-apps/ui";

export interface MsisdnDisplayProps {
  /** E.164, e.g. `+237677123456` — `Message.msisdn`'s own stored shape. */
  value: string;
  /** `Message.operator` — an `OperatorCode` string, accepted loosely
   * rather than importing `@vsms/gateway`'s type, so a gallery or a test
   * can pass a literal. */
  operator?: string | undefined;
  className?: string | undefined;
}

const OPERATOR_TAGS: Record<string, string> = {
  mtn: "MTN",
  orange: "ORG",
  camtel: "CMT",
  nexttel: "NXT",
};

/**
 * `+237 6 77 12 34 56` — the Cameroon convention of a country code, then a
 * leading digit plus four pairs.
 *
 * Only formats the shape `sms-msisdn` actually produces for a Cameroonian
 * mobile number (`+237` + 9 digits); anything else — a foreign number now
 * that `sms-msisdn` parses every country, the 8-digit `88x` toll-free
 * exception, a malformed value — falls back to the raw string rather than
 * mis-grouping it. A mis-grouped number reads as a different number,
 * which is worse than an ungrouped one.
 *
 * This is the Cameroon-specific half of what used to be `@vaam-apps/ui`'s
 * `MsisdnDisplay`. The generic half — never truncate, copy the raw E.164
 * rather than the pretty form, keep the carrier tag column aligned — is
 * `PhoneDisplay` in that package, and this file supplies it the two
 * things a component library cannot know: how this market groups its
 * digits, and what its carriers are called.
 */
function formatE164Cameroon(raw: string): string | null {
  const digits = raw.replace(/[^\d]/g, "");
  if (!digits.startsWith("237") || digits.length !== 12) return null;
  const rest = digits.slice(3);
  const lead = rest.slice(0, 1);
  const pairs = rest.slice(1).match(/.{1,2}/g);
  if (pairs === null || pairs.length !== 4) return null;
  return `+237 ${lead} ${pairs.join(" ")}`;
}

export function MsisdnDisplay({ value, operator, className }: MsisdnDisplayProps) {
  return (
    <PhoneDisplay
      value={value}
      format={formatE164Cameroon}
      tag={operator !== undefined ? OPERATOR_TAGS[operator] : undefined}
      tagTitle="Inferred from prefix — not authoritative."
      className={className}
    />
  );
}
