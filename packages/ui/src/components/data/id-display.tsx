"use client";

import { Check, Copy } from "lucide-react";
import { useState } from "react";
import { cn } from "../../lib/cn";

export interface IdDisplayProps {
  value: string;
  /** Table variant (default): first 7 chars + a hover-reveal copy button —
   * "enough to disambiguate visually within one screen of rows" (design
   * doc §7.3). `full`: the complete `cs_cuid()` (23 chars), selectable,
   * always-visible copy — detail views only. Copy always copies the FULL
   * value, never the truncation, in both variants. */
  variant?: "table" | "full";
  className?: string;
}

/**
 * `cs_cuid()` display per the design doc's data-display rules (§7.3):
 * never truncate in the middle (no `abc…xyz` — a middle ellipsis makes two
 * ids that differ only in the middle look identical), never decorate with
 * a prefix (a displayed `msg_abc…` pasted into a REST `?id=` filter 400s —
 * `Cuid` is format-guarded `[a-z0-9]{2,32}`).
 */
export function IdDisplay({ value, variant = "table", className }: IdDisplayProps) {
  const [copied, setCopied] = useState(false);

  async function copy() {
    await navigator.clipboard.writeText(value);
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  }

  const shown = variant === "table" ? value.slice(0, 7) : value;

  return (
    <span
      className={cn("group inline-flex items-center gap-1.5 font-mono tabular-nums", className)}
    >
      <span className={variant === "full" ? "select-all" : undefined} title={value}>
        {shown}
      </span>
      <button
        type="button"
        onClick={copy}
        aria-label={`Copy ${value}`}
        className={cn(
          "shrink-0 text-subtle-foreground transition-opacity hover:text-foreground",
          variant === "table" && "opacity-0 focus:opacity-100 group-hover:opacity-100",
        )}
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
