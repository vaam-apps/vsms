import { cn } from "../../lib/cn";
import { CopyButton } from "./copy-button";

export interface IdDisplayProps {
  value: string;
  /** `table` (default): a truncated prefix plus a hover-reveal copy
   * button — enough to disambiguate visually within one screen of rows.
   * `full`: the whole value, selectable, with an always-visible copy —
   * detail views only. Copy always copies the FULL value in both. */
  variant?: "table" | "full";
  /** How many leading characters the `table` variant shows. Seven is the
   * default because it is what makes two 23-character CUIDs distinguishable
   * in a list without dominating the column; an id family with a shared
   * prefix (`ord_`, `pi_`) needs more. */
  truncateTo?: number;
  className?: string | undefined;
}

/**
 * An opaque machine identifier — a CUID, a ULID, a provider's reference.
 *
 * Two rules, both learned the hard way and both enforced here rather than
 * left to each call site:
 *
 * - **Never truncate in the middle.** `abc…xyz` makes two ids that differ
 *   only in the middle look identical, which is precisely the comparison
 *   a human scanning a column is making.
 * - **Never decorate.** A displayed `msg_abc…` gets pasted into a filter
 *   that expects the bare value and returns nothing, or worse, a 400. If
 *   the prefix is part of the id, it is part of `value`.
 *
 * For a value that must be read in full — a config key, a role name, an
 * enum literal — use `Code` instead. This component's truncation is wrong
 * for anything a human is meant to read rather than match.
 */
export function IdDisplay({ value, variant = "table", truncateTo = 7, className }: IdDisplayProps) {
  const shown = variant === "table" ? value.slice(0, truncateTo) : value;

  return (
    <span
      className={cn("group inline-flex items-center gap-1.5 font-mono tabular-nums", className)}
    >
      <span className={variant === "full" ? "select-all" : undefined} title={value}>
        {shown}
      </span>
      <CopyButton value={value} revealOnGroupHover={variant === "table"} />
    </span>
  );
}
