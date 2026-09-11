"use client";

import { Eye, EyeOff } from "lucide-react";
import { useState } from "react";
import { cn } from "../../lib/cn";
import { CopyButton } from "./copy-button";

export interface MaskedValueProps {
  /** The full value. Present in the DOM only while revealed. */
  value: string;
  /** Trailing characters left visible while masked, so the value stays
   * recognisable without being readable. `0` masks everything. */
  reveal?: number;
  /** Leading characters left visible — for a prefixed credential like
   * `whsec_…` or `sk_live_…`, where the prefix says what kind of thing it
   * is and is not itself secret. */
  prefix?: number;
  /** Allow revealing at all. `false` gives a permanently masked value with
   * no toggle — for something displayed for recognition only. */
  revealable?: boolean;
  /** Offer a copy button. Copies the full value whether or not it is
   * currently revealed — the point of masking is the screen, not the
   * clipboard. */
  copyable?: boolean;
  /** Names the value in the toggle's and copy button's accessible names,
   * e.g. `"signing secret"`. */
  label?: string;
  className?: string | undefined;
}

const DOT = "•";

function mask(value: string, prefix: number, reveal: number): string {
  const head = value.slice(0, prefix);
  const tail = reveal > 0 ? value.slice(-reveal) : "";
  const hiddenCount = Math.max(value.length - head.length - tail.length, 0);
  // A fixed run of dots, not one per character: the exact length of a
  // secret is information, and padding it out to the real length leaks it
  // to anyone looking over a shoulder.
  const dots = DOT.repeat(hiddenCount === 0 ? 0 : Math.min(Math.max(hiddenCount, 6), 12));
  return `${head}${dots}${tail}`;
}

/**
 * A secret or sensitive value, masked by default with an explicit reveal.
 *
 * # What this is and is not
 *
 * It is a **shoulder-surfing and screen-share control**, not a security
 * boundary. Whoever renders this already received the value over the
 * wire; anyone with the session and dev tools can read it regardless.
 * Withholding data that has already crossed the wire to an authorised,
 * authenticated session would be theatre. What masking buys is real but
 * narrow: the value does not end up in a screenshot, a recording, or the
 * eyeline of the person sitting behind the operator.
 *
 * If a value must genuinely never reach the client, that is an API
 * decision — do not send it — and no component can substitute for it.
 *
 * # Why it is shared
 *
 * Both applications this package serves had hand-rolled one: a console
 * masking a webhook signing secret as `whsec_••••••••••<last 4>`, and a
 * payments dashboard rendering a masked payer reference and a TOTP
 * enrolment secret as plain text because no such component existed.
 */
export function MaskedValue({
  value,
  reveal = 4,
  prefix = 0,
  revealable = true,
  copyable = true,
  label = "value",
  className,
}: MaskedValueProps) {
  const [revealed, setRevealed] = useState(false);
  const shown = revealed ? value : mask(value, prefix, reveal);

  return (
    <span className={cn("group inline-flex items-center gap-1.5", className)}>
      <span className={cn("font-mono", revealed && "select-all break-all")}>{shown}</span>
      {revealable && (
        <button
          type="button"
          onClick={() => setRevealed((previous) => !previous)}
          aria-label={revealed ? `Hide ${label}` : `Reveal ${label}`}
          aria-pressed={revealed}
          className="shrink-0 text-subtle-foreground transition-colors hover:text-foreground"
        >
          {revealed ? <EyeOff size={12} strokeWidth={1.5} /> : <Eye size={12} strokeWidth={1.5} />}
        </button>
      )}
      {copyable && <CopyButton value={value} label={`Copy ${label}`} />}
    </span>
  );
}
