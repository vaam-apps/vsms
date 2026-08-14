import type { ReactNode } from "react";

/**
 * The outer page shell every top-level console screen wraps its content in
 * (`mx-auto max-w-[1400px]` + the console's own vertical rhythm/padding) —
 * extracted because R6 (AGENTS.md) forbids a `<name>-screen.tsx` smart
 * component from carrying any `className`/raw markup, and this exact
 * `<main className="mx-auto flex max-w-[1400px] ...">` string was
 * independently duplicated in `providers-screen.tsx`, `routes-screen.tsx`,
 * `webhooks-screen.tsx`, and `sender-ids-screen.tsx` before this extraction
 * — a second and third screen already needing it is R6's own test for
 * "belongs in `frontends/packages/ui`," not a route-local component.
 */
export function ScreenShell({ children }: { children: ReactNode }) {
  return (
    <main className="mx-auto flex max-w-[1400px] flex-col gap-6 px-4 py-6 sm:px-6 sm:py-10">
      {children}
    </main>
  );
}
