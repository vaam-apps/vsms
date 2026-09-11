"use client";

import { ChevronLeft, ChevronRight } from "lucide-react";
import { DayPicker, type DayPickerProps } from "react-day-picker";
import { cn } from "../../lib/cn";

export type CalendarProps = DayPickerProps & { className?: string | undefined };

/**
 * `react-day-picker`, themed with this design system's own tokens.
 *
 * # Why the whole `classNames` map is spelled out
 *
 * `react-day-picker` ships a stylesheet (`react-day-picker/style.css`)
 * built on its own `--rdp-*` variables. Importing it would put a second,
 * independent theme in the page whose colours are set from somewhere
 * other than `theme.css` — so the first time anyone changed a surface
 * colour, the calendar would not follow, and the mismatch would be
 * invisible until someone opened a date picker.
 *
 * The stylesheet is therefore **not** imported, and every element gets a
 * class from the tokens here instead. That is more code than a handful of
 * overrides, and it is the point: there is exactly one place this
 * component's colours come from, and it is the same place every other
 * component's come from.
 *
 * # Layout is deliberately not themed away
 *
 * The grid itself is a `<table>` and needs `border-collapse` plus fixed
 * cell sizing to stay square; those are structural, not thematic, and
 * live here rather than in a token.
 */
export function Calendar({
  className,
  classNames,
  showOutsideDays = true,
  ...props
}: CalendarProps) {
  return (
    <DayPicker
      showOutsideDays={showOutsideDays}
      className={cn("p-3", className)}
      classNames={{
        root: "text-body text-foreground",
        months: "flex flex-col gap-4 sm:flex-row",
        month: "flex flex-col gap-3",
        month_caption: "flex h-8 items-center justify-center",
        caption_label: "font-medium text-body text-foreground",
        nav: "flex items-center gap-1",
        // The nav buttons are absolutely positioned by RDP's own layout at
        // the top corners of the month; these restyle them in place.
        button_previous: cn(
          "absolute top-3 left-3 inline-flex size-7 items-center justify-center rounded-sm",
          "text-muted-foreground transition-colors hover:bg-surface-3 hover:text-foreground",
          "disabled:pointer-events-none disabled:opacity-40",
        ),
        button_next: cn(
          "absolute top-3 right-3 inline-flex size-7 items-center justify-center rounded-sm",
          "text-muted-foreground transition-colors hover:bg-surface-3 hover:text-foreground",
          "disabled:pointer-events-none disabled:opacity-40",
        ),
        month_grid: "w-full border-collapse",
        weekdays: "flex",
        weekday: "w-8 font-normal text-caption text-subtle-foreground",
        week: "mt-1 flex w-full",
        // Selection state lives on the CELL, not on the button inside it.
        // Read off the real DOM, not guessed: react-day-picker puts
        // `aria-selected` and `data-selected` on the `<td>` and leaves the
        // `<button>` carrying only `type`/`tabindex`/`aria-label`. An
        // earlier revision of this file styled the button with
        // `aria-selected:bg-primary` and the cell with
        // `[&:has([data-range-middle])]`, and BOTH silently matched
        // nothing — no selected day had a fill and no range had a tint.
        // Neither `tsc` nor the classNames-key test could see it: the keys
        // were all real, and the class strings were valid CSS for a DOM
        // shape that does not exist. Found by rendering it.
        day: "relative size-8 p-0 text-center",
        day_button: cn(
          "inline-flex size-8 items-center justify-center rounded-sm font-mono text-caption tabular-nums",
          "text-foreground transition-colors hover:bg-surface-3",
          "focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring",
        ),
        // Fills the button from the cell's own state, which is where the
        // state actually is. Covers single mode outright and both ends of
        // a range; `range_middle` below overrides it for the interior.
        selected:
          "[&>button]:bg-primary [&>button]:text-primary-content [&>button]:hover:bg-primary",
        today: "font-semibold text-state-uncertain-fg",
        outside: "text-subtle-foreground opacity-50",
        disabled: "pointer-events-none opacity-30",
        hidden: "invisible",
        // The interior of a range: tint the CELL so consecutive days meet
        // with no gap, and knock the button's own fill back out.
        //
        // Two things here were wrong on the first attempt and are worth
        // keeping written down. The tint was `bg-state-neutral-bg`, which
        // is a real token that resolves to `transparent` — `neutral` is
        // the quiet hue and deliberately has no fill — so the interior
        // rendered as nothing at all. It is a wash of the selection
        // colour instead, which is also what it means. And the `!` is
        // load-bearing: a middle day also carries `selected` above, and
        // both rules are arbitrary variants of equal specificity, so
        // without it the winner is whichever class Tailwind emits last.
        range_middle:
          "bg-primary/15 [&>button]:bg-transparent! [&>button]:text-foreground! [&>button]:hover:bg-surface-3!",
        range_start: "rounded-l-sm",
        range_end: "rounded-r-sm",
        week_number: "w-8 font-mono text-caption text-subtle-foreground",
        footer: "pt-2 text-caption text-muted-foreground",
        ...classNames,
      }}
      components={{
        // RDP renders one `Chevron` for both directions and flips it with
        // `orientation`; swapping in two real icons reads better than a
        // CSS rotation and keeps the stroke weight matching every other
        // icon in the system.
        Chevron: ({ orientation, ...iconProps }) =>
          orientation === "left" ? (
            <ChevronLeft size={16} strokeWidth={1.5} {...iconProps} />
          ) : (
            <ChevronRight size={16} strokeWidth={1.5} {...iconProps} />
          ),
      }}
      {...props}
    />
  );
}
