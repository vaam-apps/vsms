/**
 * Money formatting from minor units, without losing precision.
 *
 * # Why this exists
 *
 * Money is the single most duplicated formatter in every application that
 * handles it. The sibling payments codebase this package was generalised
 * for had **four** independent implementations at the time of writing, in
 * four packages, with four different signatures and three different
 * hard-coded tables of which currencies have no decimal places — and one
 * of them divided by a power of ten in floating point, which silently
 * corrupts any amount past 2^53 minor units. All four also got the
 * three-decimal currencies wrong, because every hand-written table
 * remembers `JPY` and `XAF` and forgets `KWD` and `BHD`.
 *
 * # Three decisions, each load-bearing
 *
 * **Minor units in, never a decimal number.** An amount is an integer
 * count of the currency's smallest unit — that is how it is stored, how
 * it is transmitted, and the only representation that cannot be wrong by
 * a rounding error. `bigint` and a decimal-digit `string` are accepted
 * alongside `number` so an `i64` from a Rust backend survives the trip;
 * JSON gives it to you as a string precisely because `number` cannot hold
 * it.
 *
 * **The exponent comes from `Intl`, not from a table here.** `Intl`
 * already ships the ISO 4217 exponent for every currency, so there is no
 * list to maintain and no currency this gets wrong. Verified rather than
 * assumed: `XAF`/`XOF`/`JPY`/`CLP`/`VND` resolve to 0 and `KWD`/`BHD` to
 * 3, which is exactly the set hand-written tables miss.
 *
 * **The scaling is string surgery, never division.** The minor-unit
 * integer is split into whole and fractional parts by digit position and
 * handed to `Intl.NumberFormat` as a string, which accepts arbitrary
 * precision. No value is ever converted to a float, so an amount larger
 * than `Number.MAX_SAFE_INTEGER` formats exactly.
 */

/** An integer count of a currency's smallest unit. */
export type MinorUnits = number | bigint | string;

export interface FormatMoneyOptions {
  /** BCP 47 tag. Defaults to the runtime's own locale.
   *
   * Pass one explicitly anywhere the output must be stable — a test, a
   * server-rendered page whose HTML must match the client's hydration, or
   * a value a human will compare against a bank statement. */
  locale?: string | undefined;
  /** `symbol` → `FCFA 5,000`; `code` → `XAF 5,000`; `none` → `5,000`.
   *
   * `code` is the honest default for an operator console: currency
   * symbols are ambiguous across locales ($ alone names at least a dozen
   * currencies) and an operator reconciling against a ledger needs the
   * ISO code. */
  display?: "symbol" | "code" | "none";
  /** Force a sign even when positive, e.g. for a ledger delta. */
  signDisplay?: Intl.NumberFormatOptions["signDisplay"];
}

const DIGITS_ONLY = /^-?\d+$/;

/** Cache: constructing an `Intl.NumberFormat` is the expensive part, and a
 * table renders the same currency once per row. */
const formatters = new Map<string, Intl.NumberFormat>();

function formatter(key: string, options: Intl.NumberFormatOptions, locale?: string) {
  const cacheKey = `${locale ?? ""}|${key}`;
  let found = formatters.get(cacheKey);
  if (found === undefined) {
    found = new Intl.NumberFormat(locale, options);
    formatters.set(cacheKey, found);
  }
  return found;
}

/**
 * How many decimal places this currency has, per ISO 4217, as `Intl`
 * knows it. `2` for a currency `Intl` has never heard of — the majority
 * case, and the same fallback `Intl` itself uses.
 */
export function currencyExponent(currency: string, locale?: string): number {
  return (
    formatter(`exp:${currency}`, { style: "currency", currency }, locale).resolvedOptions()
      .maximumFractionDigits ?? 2
  );
}

/**
 * Shifts a minor-unit integer right by `exponent` places, as a string.
 *
 * `"500"`, 2 → `"5.00"`; `"5"`, 2 → `"0.05"`; `"-5"`, 2 → `"-0.05"`;
 * `"5000"`, 0 → `"5000"`.
 */
function shiftDecimal(digits: string, exponent: number): string {
  const negative = digits.startsWith("-");
  const abs = negative ? digits.slice(1) : digits;
  const sign = negative ? "-" : "";
  if (exponent === 0) return sign + abs;
  const padded = abs.padStart(exponent + 1, "0");
  const whole = padded.slice(0, padded.length - exponent);
  const fraction = padded.slice(padded.length - exponent);
  return `${sign}${whole}.${fraction}`;
}

/**
 * `formatMoney(500000, "XAF")` → `"XAF 500,000"`.
 * `formatMoney("1250", "USD")` → `"USD 12.50"`.
 *
 * @throws if `minorUnits` is not an integer. A non-integer minor-unit
 * amount is always a bug — usually a caller that already divided — and
 * silently rounding it here would hide the real error at the point where
 * it is least visible.
 */
export function formatMoney(
  minorUnits: MinorUnits,
  currency: string,
  options: FormatMoneyOptions = {},
): string {
  const { locale, display = "code", signDisplay } = options;

  let digits: string;
  if (typeof minorUnits === "bigint") {
    digits = minorUnits.toString();
  } else if (typeof minorUnits === "number") {
    if (!Number.isInteger(minorUnits)) {
      throw new TypeError(
        `formatMoney expects an integer count of minor units, got ${minorUnits} (${currency}). ` +
          "Pass 1250 for USD 12.50, not 12.5.",
      );
    }
    digits = minorUnits.toFixed(0);
  } else {
    const trimmed = minorUnits.trim();
    if (!DIGITS_ONLY.test(trimmed)) {
      throw new TypeError(
        `formatMoney expects an integer count of minor units, got ${JSON.stringify(minorUnits)} (${currency}).`,
      );
    }
    digits = trimmed;
  }

  const exponent = currencyExponent(currency, locale);
  const value = shiftDecimal(digits, exponent);

  const intlOptions: Intl.NumberFormatOptions =
    display === "none"
      ? {
          style: "decimal",
          minimumFractionDigits: exponent,
          maximumFractionDigits: exponent,
        }
      : {
          style: "currency",
          currency,
          currencyDisplay: display,
        };
  if (signDisplay !== undefined) intlOptions.signDisplay = signDisplay;

  return formatter(
    `fmt:${currency}:${display}:${signDisplay ?? ""}:${exponent}`,
    intlOptions,
    locale,
  ).format(value as unknown as number);
}
