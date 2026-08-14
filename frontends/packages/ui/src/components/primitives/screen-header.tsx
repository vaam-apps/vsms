import type { ReactNode } from "react";

/**
 * The "vsms admin console" eyebrow + `h1` + one-line description masthead
 * every top-level screen repeats verbatim (only `title`/`description`
 * differ) — see `ScreenShell`'s own doc comment for why this was pulled out
 * of the individual screens rather than left duplicated a fourth time.
 */
export function ScreenHeader({
  title,
  description,
}: {
  title: ReactNode;
  description?: ReactNode;
}) {
  return (
    <header className="flex flex-col gap-1 border-edge border-b pb-6">
      <p className="font-mono text-micro text-subtle-foreground tracking-[0.03em]">
        vsms admin console
      </p>
      <h1 className="font-medium text-foreground text-title">{title}</h1>
      {description != null && (
        <p className="max-w-xl text-body text-muted-foreground">{description}</p>
      )}
    </header>
  );
}
