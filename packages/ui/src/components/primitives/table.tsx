import type { HTMLAttributes, TdHTMLAttributes, ThHTMLAttributes } from "react";
import { cn } from "../../lib/cn";

// Table conventions per design doc §6.4: sticky header on surface-1, 1px
// `border-edge-subtle` row dividers, no zebra striping (the status tints
// are the signal — zebra plus tints is noise), hover on surface-3.

export function Table({ className, ...props }: HTMLAttributes<HTMLTableElement>) {
  return (
    <div className="w-full overflow-x-auto">
      <table className={cn("w-full border-collapse text-body", className)} {...props} />
    </div>
  );
}

export function TableHeader({ className, ...props }: HTMLAttributes<HTMLTableSectionElement>) {
  return (
    <thead
      className={cn("sticky top-0 z-10 bg-surface-1 [&_tr]:border-b [&_tr]:border-edge", className)}
      {...props}
    />
  );
}

export function TableBody({ className, ...props }: HTMLAttributes<HTMLTableSectionElement>) {
  return <tbody className={cn("[&_tr:last-child]:border-b-0", className)} {...props} />;
}

export interface TableRowProps extends HTMLAttributes<HTMLTableRowElement> {
  selected?: boolean;
}

export function TableRow({ className, selected = false, ...props }: TableRowProps) {
  return (
    <tr
      className={cn(
        "border-edge-subtle border-b transition-colors duration-[var(--dur-instant)] hover:bg-surface-3",
        selected && "bg-surface-3 shadow-[inset_2px_0_0_var(--color-ring)]",
        className,
      )}
      {...props}
    />
  );
}

export interface TableHeadProps extends Omit<ThHTMLAttributes<HTMLTableCellElement>, "align"> {
  align?: "start" | "end";
}

export function TableHead({ className, align = "start", ...props }: TableHeadProps) {
  return (
    <th
      className={cn(
        "h-8 whitespace-nowrap px-3 font-medium text-micro text-muted-foreground tracking-[0.03em]",
        align === "end" ? "text-right" : "text-left",
        className,
      )}
      {...props}
    />
  );
}

export interface TableCellProps extends Omit<TdHTMLAttributes<HTMLTableCellElement>, "align"> {
  align?: "start" | "end";
  mono?: boolean;
}

export function TableCell({ className, align = "start", mono = false, ...props }: TableCellProps) {
  return (
    <td
      className={cn(
        "px-3 py-2 text-body",
        align === "end" ? "text-right" : "text-left",
        mono && "font-mono tabular-nums",
        className,
      )}
      {...props}
    />
  );
}
