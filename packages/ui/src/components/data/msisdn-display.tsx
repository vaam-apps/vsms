"use client";

import { Check, Copy } from "lucide-react";
import { useState } from "react";
import { cn } from "../../lib/cn";

export interface MsisdnDisplayProps {
  /** E.164, e.g. `+237677123456` — `Message.msisdn`'s own stored shape. */
  value: string;
  /** `Message.operator` — an `OperatorCode` string, but this package takes
   * zero internal deps, so it's accepted loosely rather than importing
   * `@vsms/gateway`'s type. */
  operator?: string | undefined;
  className?: string;
}

const OPERATOR_TAGS: Record<string, string> = {
  mtn: "MTN",
  orange: "ORG",
  camtel: "CMT",
  nexttel: "NXT",
};

/**
 * `+237 6 77 12 34 56` — country code, then the Cameroon convention of a
 * leading digit plus four pairs (design doc §7.1). Only formats the
 * shape `sms-msisdn` actually produces for a mobile number (`+237` + 9
 * digits); anything else — the 8-digit `88x` toll-free exception, a
 * malformed value — falls back to showing the raw string verbatim rather
 * than mis-grouping it. Full classification (`unallocated` vs a genuine
 * parse failure, the `88x` exception) is `sms-msisdn`'s own domain,
 * mirrored here only for the one shape the messages list actually shows
 * today: a real mobile MSISDN a message was accepted for.
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

/** Never truncated (design doc §7.1: "a half-shown phone number is worse
 * than useless in an investigation"). Copy always copies the raw,
 * unformatted E.164 value — the form that pastes into a query. */
export function MsisdnDisplay({ value, operator, className }: MsisdnDisplayProps) {
  const [copied, setCopied] = useState(false);
  const formatted = formatE164Cameroon(value) ?? value;
  const tag = operator !== undefined ? OPERATOR_TAGS[operator] : undefined;

  async function copy() {
    await navigator.clipboard.writeText(value);
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  }

  return (
    <span className={cn("group inline-flex items-center gap-1.5 whitespace-nowrap", className)}>
      <span className="font-mono tabular-nums">{formatted}</span>
      <span
        className="font-mono text-[11px] text-subtle-foreground"
        title="Inferred from prefix — not authoritative."
      >
        {tag ?? "—"}
      </span>
      <button
        type="button"
        onClick={copy}
        aria-label={`Copy ${value}`}
        className="shrink-0 text-subtle-foreground opacity-0 transition-opacity hover:text-foreground focus:opacity-100 group-hover:opacity-100"
      >
        {copied ? (
          <Check size={12} strokeWidth={1.5} className="text-state-success-fg" />
        ) : (
          <Copy size={12} strokeWidth={1.5} />
        )}
      </button>
    </span>
  );
}
