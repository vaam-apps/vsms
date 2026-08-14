import type { ReactNode } from "react";
import { cn } from "../../lib/cn";

// R6 (AGENTS.md): every route screen was hand-rolling the identical
// `<div className="flex flex-col gap-6">` wrapper plus a
// `<h1 className="font-medium text-foreground text-title">`/description
// `<header>` pair — confirmed by grep, ten of eleven screens under
// `frontends/apps/admin/app/*/*.tsx` had this exact pair before this file
// existed. R6's own test ("if a second route would plausibly use it, it
// belongs in `frontends/packages/ui`") is not a close call here. Extracted
// verbatim from `jobs-screen.tsx`/`workers-screen.tsx`/`opt-outs-screen.tsx`
// — same classes, not a redesign.
//
// New file, added while several route groups were being moved onto R6 in
// parallel — expect another agent's PR to add an equivalent. If both land,
// reconcile onto one of the two rather than keeping both: same shape,
// different name is worse than either alone.

export interface ScreenStackProps {
  children: ReactNode;
  className?: string;
}

/** The page-level vertical rhythm every screen composes its sections into. */
export function ScreenStack({ children, className }: ScreenStackProps) {
  return <div className={cn("flex flex-col gap-6", className)}>{children}</div>;
}

export interface ScreenHeaderProps {
  title: ReactNode;
  description?: ReactNode;
}

/** A screen's own `<h1>` + one-line description, styled once. */
export function ScreenHeader({ title, description }: ScreenHeaderProps) {
  return (
    <header className="flex flex-col gap-1">
      <h1 className="font-medium text-foreground text-title">{title}</h1>
      {description != null && (
        <p className="max-w-xl text-body text-muted-foreground">{description}</p>
      )}
    </header>
  );
}
