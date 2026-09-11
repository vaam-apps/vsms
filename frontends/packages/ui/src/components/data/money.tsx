import { cn } from "../../lib/cn";
import { type FormatMoneyOptions, formatMoney, type MinorUnits } from "../../lib/money";

export interface MoneyProps extends FormatMoneyOptions {
  /** An integer count of the currency's smallest unit. */
  amount: MinorUnits;
  /** ISO 4217 code, e.g. `"XAF"`. */
  currency: string;
  /** Tint by sign: credits green, debits red. Off by default — in a
   * ledger where most rows are one direction, colouring every row is
   * noise, and colour alone is not an accessible way to convey sign. The
   * minus sign is always rendered regardless. */
  tone?: "none" | "signed";
  className?: string | undefined;
}

/**
 * An amount of money, right-alignable and vertically comparable.
 *
 * `tabular-nums` is the whole reason this is a component and not a call to
 * [`formatMoney`] in a `<span>`: without fixed-width digits, a column of
 * amounts does not line up at the decimal point, and scanning it for an
 * outlier — the actual job — stops working.
 *
 * `<data value>` rather than `<span>`: the machine-readable amount stays
 * in the DOM in minor units, so a copy-paste or a scraper gets the exact
 * integer rather than a locale-formatted string it would have to parse
 * back.
 */
export function Money({ amount, currency, tone = "none", className, ...options }: MoneyProps) {
  const formatted = formatMoney(amount, currency, options);
  const negative = String(amount).trim().startsWith("-");

  return (
    <data
      value={`${String(amount)} ${currency}`}
      className={cn(
        "font-mono tabular-nums whitespace-nowrap",
        tone === "signed" && (negative ? "text-state-danger-fg" : "text-state-success-fg"),
        className,
      )}
    >
      {formatted}
    </data>
  );
}
