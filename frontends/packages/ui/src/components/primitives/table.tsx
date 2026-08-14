import type { HTMLAttributes, TdHTMLAttributes, ThHTMLAttributes } from "react";
import { cn } from "../../lib/cn";

// Table conventions per design doc §6.4: sticky header on surface-1, 1px
// `border-edge-subtle` row dividers, no zebra striping (the status tints
// are the signal — zebra plus tints is noise), hover on surface-3.
//
// D16: daisyUI's real `table`/`table-pin-rows` classes sit underneath this
// file's own bespoke row/cell behaviour (constraint 7 — "DaisyUI does the
// work") rather than replacing it. Every own-authored utility class below
// is unchanged from before this PR; `table`/`table-pin-rows` are added on
// top and reconciled against three things confirmed by reading
// `daisyui/components/table.css` directly, not assumed:
//
// 1. `table-pin-rows` alone does nothing — every one of its selectors is
//    `.table :where(.table-pin-rows thead)`, i.e. it only matches when
//    `table-pin-rows` sits on the same element as `table`. Both classes go
//    on `<table>` together, never one without the other.
// 2. `.table` switches `border-collapse` to `separate`. Under the separate
//    border model a border set on `<tr>` — this file's whole row-divider
//    mechanism, `TableRow`'s `border-edge-subtle border-b` — does not
//    render at all; the CSS spec confines borders in that model to cells,
//    never rows or sections. `border-collapse!` (Tailwind v4's trailing-`!`
//    important syntax) stays on `<table>` specifically so adding
//    `.table` doesn't silently delete every row divider in this app —
//    verified live against a real render (`just demo`, `/messages`) with
//    and without the override before trusting it.
// 3. `.table` also applies its own `padding-block`/`padding-inline` to
//    every `th`/`td` via a zero-specificity-inside `:where()` selector,
//    and its own `font-size`. `TableHead`/`TableCell` keep their existing
//    `px-3`/`py-2`/`h-8` utilities unchanged, at equal selector
//    specificity to daisyUI's rule — confirmed live that the existing
//    cell padding and type scale still win, not daisyUI's defaults.
export function Table({ className, ...props }: HTMLAttributes<HTMLTableElement>) {
  return (
    <div className="w-full overflow-x-auto">
      <table
        className={cn("table table-pin-rows w-full border-collapse! text-body", className)}
        {...props}
      />
    </div>
  );
}

export function TableHeader({ className, ...props }: HTMLAttributes<HTMLTableSectionElement>) {
  return (
    <thead
      className={cn(
        "sticky top-0 z-10 bg-surface-1 [&_tr]:border-b [&_tr]:border-edge [&_tr]:bg-surface-1",
        className,
      )}
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

/**
 * Responsive column visibility. `hideBelow="md"` hides the cell on
 * viewports narrower than `md` and shows it from `md` up.
 *
 * This exists because the class it replaces was the single
 * most-duplicated string in the console: `"hidden sm:table-cell"`,
 * `"hidden md:table-cell"` and `"hidden lg:table-cell"` appeared **90
 * times** as inline literals across the route-local table components.
 *
 * Worth recording how they got there, because it is a lesson about the
 * rule and not just about tables. R6's own text names
 * `const COL_ID = "hidden lg:table-cell"` in `jobs-screen.tsx` — four
 * hoisted consts — as the motivating example of a class const that must
 * not live in a view. The R6 sweep removed those four consts and, in
 * moving the markup into dumb components, re-expressed the same decision
 * as 90 inline literals. The letter of the rule was satisfied (no classes
 * in a *view* file); the thing the rule exists to prevent — one decision
 * written in many places — got 22× worse.
 *
 * A mapping table rather than an interpolated `` `hidden ${bp}:table-cell` ``
 * because Tailwind scans source text statically: a template literal
 * produces no class at all in the built CSS. The full strings must appear
 * verbatim somewhere Tailwind can see them, and this is that place.
 */
const HIDE_BELOW: Record<Breakpoint, string> = {
  sm: "hidden sm:table-cell",
  md: "hidden md:table-cell",
  lg: "hidden lg:table-cell",
  xl: "hidden xl:table-cell",
};

export type Breakpoint = "sm" | "md" | "lg" | "xl";

export interface TableHeadProps extends Omit<ThHTMLAttributes<HTMLTableCellElement>, "align"> {
  align?: "start" | "end";
  /** Hide this column below the given breakpoint. See `HIDE_BELOW`. */
  hideBelow?: Breakpoint | undefined;
}

export function TableHead({ className, align = "start", hideBelow, ...props }: TableHeadProps) {
  return (
    <th
      className={cn(
        "h-8 whitespace-nowrap px-3 font-medium text-micro text-muted-foreground tracking-[0.03em]",
        align === "end" ? "text-right" : "text-left",
        hideBelow && HIDE_BELOW[hideBelow],
        className,
      )}
      {...props}
    />
  );
}

export interface TableCellProps extends Omit<TdHTMLAttributes<HTMLTableCellElement>, "align"> {
  align?: "start" | "end";
  mono?: boolean;
  /** Hide this column below the given breakpoint. See `HIDE_BELOW`. */
  hideBelow?: Breakpoint | undefined;
}

export function TableCell({
  className,
  align = "start",
  mono = false,
  hideBelow,
  ...props
}: TableCellProps) {
  return (
    <td
      className={cn(
        "px-3 py-2 text-body",
        align === "end" ? "text-right" : "text-left",
        mono && "font-mono tabular-nums",
        hideBelow && HIDE_BELOW[hideBelow],
        className,
      )}
      {...props}
    />
  );
}
