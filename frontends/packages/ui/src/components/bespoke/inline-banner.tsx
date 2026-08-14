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
  /** `'neutral'` — a standing notice (scope, caveat). `'danger'` — an error.
   * `'plain'` — a caption-only note with no border or fill, for something
   * that doesn't need the weight of a bordered box. */
  variant?: "neutral" | "danger" | "plain";
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
        variant === "plain" && "text-subtle-foreground",
        className,
      )}
    >
      {children}
    </div>
  );
}
