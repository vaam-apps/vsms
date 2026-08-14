// Extracted verbatim from the composer screen (R6, AGENTS.md).

/** Only include `to` in the preview call once it looks like an attempted
 * number, not a fragment of one — `previewMessage` validates a supplied
 * `to` as a real Cameroon mobile (`Msisdn::parse_mobile`) and 422s the
 * *whole call* if it doesn't parse, which would otherwise blank out the
 * encoding stats every keystroke while the operator is still typing the
 * recipient. Once it looks complete, a real invalid number still 422s —
 * `isStale` is what surfaces that, keeping the last good encoding numbers
 * on screen rather than clearing them. */
export function looksLikeAttemptedMsisdn(raw: string): boolean {
  return raw.replace(/\D/g, "").length >= 8;
}
