"use client";

import { Check, Copy } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { cn } from "../../lib/cn";

export interface CopyButtonProps {
  /** The exact text put on the clipboard. Always the full, unformatted,
   * machine-readable value — never what is displayed. A truncated id or a
   * prettily-spaced phone number pasted into a query returns nothing. */
  value: string;
  /** Accessible name. Defaults to `Copy ${value}`, which is right for a
   * short id and wrong for a paragraph — pass something shorter then. */
  label?: string | undefined;
  /** Hide until the containing `.group` is hovered or this button is
   * focused. For dense tables, where a permanently visible icon on every
   * row is noise. Keyboard users always reach it: focus reveals it. */
  revealOnGroupHover?: boolean;
  size?: 12 | 14 | 16;
  className?: string | undefined;
}

const CONFIRMATION_MS = 1500;

/**
 * Copy-to-clipboard affordance: a click, then a checkmark for a moment.
 *
 * Extracted because `IdDisplay` and `MsisdnDisplay` each carried their own
 * byte-identical copy — the same `useState`, the same 1500ms timeout, the
 * same icon swap, the same class string — and any component that later
 * wanted the behaviour would have written a third.
 *
 * Two things the duplicated versions got wrong, fixed here rather than
 * copied a third time:
 *
 * - **The timeout was never cleared.** Unmounting within the confirmation
 *   window (a row scrolling out of a virtualised table, a drawer closing)
 *   left a timer that fired `setCopied` on a dead component. Now cleared
 *   on unmount, and any in-flight timer is cleared before a new one
 *   starts, so a double-click cannot end the confirmation early.
 * - **A rejected `writeText` was unhandled.** `navigator.clipboard` is
 *   unavailable on an insecure origin and can be refused by permissions
 *   policy; the bare `await` meant an unhandled rejection and a button
 *   that silently did nothing. Failure now leaves the icon unchanged,
 *   which is at least honest, and reports the reason to the console.
 */
export function CopyButton({
  value,
  label,
  revealOnGroupHover = false,
  size = 12,
  className,
}: CopyButtonProps) {
  const [copied, setCopied] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(
    () => () => {
      if (timer.current !== null) clearTimeout(timer.current);
    },
    [],
  );

  const copy = useCallback(() => {
    void navigator.clipboard.writeText(value).then(
      () => {
        setCopied(true);
        if (timer.current !== null) clearTimeout(timer.current);
        timer.current = setTimeout(() => setCopied(false), CONFIRMATION_MS);
      },
      (error: unknown) => {
        // Insecure origin, or a permissions policy that forbids it. Leave
        // the icon alone rather than claim a copy that did not happen.
        console.error("Clipboard write refused", error);
      },
    );
  }, [value]);

  return (
    <button
      type="button"
      onClick={copy}
      aria-label={label ?? `Copy ${value}`}
      className={cn(
        "shrink-0 text-subtle-foreground transition-opacity hover:text-foreground",
        revealOnGroupHover && "opacity-0 focus-visible:opacity-100 group-hover:opacity-100",
        className,
      )}
    >
      {copied ? (
        <Check size={size} strokeWidth={1.5} className="text-state-success-fg" />
      ) : (
        <Copy size={size} strokeWidth={1.5} />
      )}
    </button>
  );
}
