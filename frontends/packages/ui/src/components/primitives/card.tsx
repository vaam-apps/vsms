import type { HTMLAttributes, ReactNode } from "react";
import { cn } from "../../lib/cn";

export type CardProps = HTMLAttributes<HTMLDivElement>;

// Borders, not shadows (design doc §3.6): every card is `--shadow-none` +
// a 1px border. daisyUI's own `card` class would add `shadow-xl` under
// some presets — explicitly opted out here rather than stripped after the
// fact, since this is a hand-built component, not a generated one.
//
// D8: the previous `rounded-sm` override is dropped, not swapped for
// `rounded-box` — daisyUI's own `.card` rule already sets
// `border-radius: var(--radius-box)` (confirmed by reading
// `daisyui/components/card.css` directly), so an explicit override was
// fighting the component's own class at equal specificity for no reason.
// This *is* the deliberate D8 value change §5's inventory calls for
// ("new radius (`--radius-box`)") — the previous `rounded-sm` resolved to
// `--radius-field` (12px) once Phase 0 rewrote the alias, one tier tighter
// than the box-tier corners the reference lock (§1.1/§1.2/§1.5) shows for
// card/drawer/panel chrome.
export function Card({ className, ...props }: CardProps) {
  return (
    <div className={cn("card border border-edge bg-base-300 shadow-none", className)} {...props} />
  );
}

export interface CardHeaderProps extends Omit<HTMLAttributes<HTMLDivElement>, "title"> {
  title: ReactNode;
  /** Optional mono metadata line beneath the title. */
  meta?: ReactNode;
  /** Right-aligned slot, e.g. a row of buttons. */
  action?: ReactNode;
}

export function CardHeader({ title, meta, action, className, ...props }: CardHeaderProps) {
  return (
    <div className={cn("flex items-start justify-between gap-4 p-4", className)} {...props}>
      <div className="min-w-0">
        <h3 className="truncate font-medium text-foreground text-title-sm">{title}</h3>
        {meta != null && (
          <p className="mt-1 truncate font-mono text-caption text-subtle-foreground">{meta}</p>
        )}
      </div>
      {action != null && <div className="shrink-0">{action}</div>}
    </div>
  );
}

export function CardBody({ className, ...props }: HTMLAttributes<HTMLDivElement>) {
  return <div className={cn("px-4 pb-4", className)} {...props} />;
}
