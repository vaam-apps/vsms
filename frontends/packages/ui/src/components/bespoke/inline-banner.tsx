import type { ReactNode } from "react";
import { cn } from "../../lib/cn";

// R6 (AGENTS.md): the same three banner treatments — a neutral scope/notice
// box, a danger error box, and a plain caption-only note — were hand-rolled
// inline in every screen that needed one (`rounded-sm border border-edge
// bg-surface-2 px-3 py-2 text-caption text-muted-foreground` for "neutral",
// `border-state-danger-border bg-state-danger-bg ... text-state-danger-fg`
// for "danger"). Classes moved here verbatim, parameterised by variant, not
// redesigned. `InlineEmptyState` already set the precedent for this shape
// of extraction (a status treatment repeated per-screen, given one home) —
// this is the same move applied to a different repeated block.
//
// Not to be confused with `InlineEmptyState`: that one is for "there is
// nothing to show here" inside a list; this one is for a standing notice or
// an error above/around content that *is* showing.

export interface InlineBannerProps {
  /** `'neutral'` — a standing notice (scope, caveat). `'danger'` — an
   * error. `'warning'` — recoverable, needs attention (a stale write).
   * `'success'` — a positive confirmation (a verified audit chain).
   * `'uncertain'` — degraded or unknown, neither failure nor success (a
   * stalled live feed, a UCS-2 volume spike). `'plain'` — a caption-only
   * note with no border or fill, for something that doesn't need the
   * weight of a bordered box.
   *
   * The last three were added after the R6 factorization, not with it:
   * four route groups had hand-rolled boxes in exactly these tones because
   * the variant they needed did not exist, and the agents doing the
   * factorization were deliberately barred from editing this package. Two
   * of them flagged the gap rather than inventing a competing component,
   * which is the behaviour the constraint was for. */
  variant?: "neutral" | "danger" | "warning" | "success" | "uncertain" | "plain";
  children: ReactNode;
  className?: string;
}

export function InlineBanner({ variant = "neutral", children, className }: InlineBannerProps) {
  return (
    <div
      className={cn(
        "text-caption",
        variant === "neutral" &&
          "rounded-sm border border-edge bg-surface-2 px-3 py-2 text-muted-foreground",
        variant === "danger" &&
          "rounded-sm border border-state-danger-border bg-state-danger-bg px-3 py-2 text-state-danger-fg",
        variant === "warning" &&
          "rounded-sm border border-state-warning-border bg-state-warning-bg px-3 py-2 text-state-warning-fg",
        variant === "success" &&
          "rounded-sm border border-state-success-border bg-state-success-bg px-3 py-2 text-state-success-fg",
        variant === "uncertain" &&
          "rounded-sm border border-state-uncertain-border bg-state-uncertain-bg px-3 py-2 text-state-uncertain-fg",
        variant === "plain" && "text-subtle-foreground",
        className,
      )}
    >
      {children}
    </div>
  );
}
