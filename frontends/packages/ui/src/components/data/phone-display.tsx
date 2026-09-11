import { cn } from "../../lib/cn";
import { CopyButton } from "./copy-button";

export interface PhoneDisplayProps {
  /** E.164, e.g. `+237677123456`. Stored form, and what gets copied. */
  value: string;
  /**
   * Regroups the E.164 digits for reading. Return `null` — or omit the
   * prop — to render the value verbatim.
   *
   * Grouping is a per-country convention (`+237 6 77 12 34 56` in
   * Cameroon, `+1 (415) 555-0132` in the NANP) and getting it wrong is
   * worse than not grouping at all, because a mis-grouped number reads as
   * a different number. This component therefore ships no default
   * grouping and no built-in country table: a caller that knows its
   * market passes a formatter, and everyone else gets the raw E.164,
   * which is always correct if less pretty.
   */
  format?: ((e164: string) => string | null) | undefined;
  /**
   * A short mono tag after the number — a carrier, a line type, a region.
   * Renders an em dash when absent, so a column of numbers stays aligned
   * whether or not each row could be classified.
   */
  tag?: string | undefined;
  /** Hover text for the tag. Say where it came from and how much to trust
   * it: a carrier inferred from a number prefix is a guess, and number
   * portability means it is wrong for a real fraction of subscribers. */
  tagTitle?: string | undefined;
  className?: string | undefined;
}

/**
 * A phone number, never truncated.
 *
 * Truncation is banned rather than discouraged: a half-shown phone number
 * is worse than useless in an investigation, and unlike an opaque id
 * (where a prefix is enough to match rows by eye) there is no prefix of a
 * phone number that identifies it.
 *
 * Copy always copies the raw E.164 — the form that pastes into a query —
 * not the grouped form on screen.
 *
 * Previously this was `MsisdnDisplay`, with Cameroon's `+237` grouping and
 * a four-entry Cameroonian carrier table compiled in. Both are now the
 * caller's: an application knows its market, a component library does not.
 */
export function PhoneDisplay({ value, format, tag, tagTitle, className }: PhoneDisplayProps) {
  const formatted = format?.(value) ?? value;

  return (
    <span className={cn("group inline-flex items-center gap-1.5 whitespace-nowrap", className)}>
      <span className="font-mono tabular-nums">{formatted}</span>
      <span className="font-mono text-micro text-subtle-foreground" title={tagTitle}>
        {tag ?? "—"}
      </span>
      <CopyButton value={value} revealOnGroupHover />
    </span>
  );
}
