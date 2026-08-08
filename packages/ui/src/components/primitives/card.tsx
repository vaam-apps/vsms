import type { HTMLAttributes, ReactNode } from "react";
import { cn } from "../../lib/cn";

export type CardProps = HTMLAttributes<HTMLDivElement>;

// Borders, not shadows (design doc §3.6): every card is `--shadow-none` +
// a 1px border. daisyUI's own `card` class would add `shadow-xl` under
// some presets — explicitly opted out here rather than stripped after the
// fact, since this is a hand-built component, not a generated one.
export function Card({ className, ...props }: CardProps) {
  return (
    <div
      className={cn("card rounded-sm border border-edge bg-base-300 shadow-none", className)}
      {...props}
    />
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
