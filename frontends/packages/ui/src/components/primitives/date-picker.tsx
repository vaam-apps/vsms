"use client";

import { Popover as HeadlessPopover, PopoverButton, PopoverPanel } from "@headlessui/react";
import { CalendarDays, X } from "lucide-react";
import type { DateRange } from "react-day-picker";
import { cn } from "../../lib/cn";
import { omitUndefined } from "../../lib/omit-undefined";
import { Calendar } from "./calendar";

export type { DateRange };

/**
 * `2026-09-11` — ISO 8601 calendar date, no time, no zone.
 *
 * Every date this module exchanges with a caller is one of these, not a
 * `Date`. A `Date` is an instant, and an instant rendered in a different
 * zone is a different calendar day — which is how a filter for "today"
 * silently returns yesterday's rows for anyone west of the server. The
 * `Date` objects `react-day-picker` works in are confined to this file.
 */
export type IsoDate = string;

/** `Date` (local midnight) → `YYYY-MM-DD`, without going through UTC. */
export function toIsoDate(date: Date): IsoDate {
  const year = date.getFullYear();
  const month = String(date.getMonth() + 1).padStart(2, "0");
  const day = String(date.getDate()).padStart(2, "0");
  return `${year}-${month}-${day}`;
}

/**
 * `YYYY-MM-DD` → `Date` at *local* midnight.
 *
 * Deliberately not `new Date("2026-09-11")`, which the spec parses as
 * UTC midnight — in any negative-offset zone that is the previous day,
 * so a calendar built on it highlights the wrong cell.
 */
export function fromIsoDate(iso: IsoDate): Date | undefined {
  const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(iso);
  if (match === null) return undefined;
  const [, year, month, day] = match as unknown as [string, string, string, string];
  return new Date(Number(year), Number(month) - 1, Number(day));
}

function displayLabel(iso: IsoDate | undefined, placeholder: string): string {
  return iso === undefined || iso === "" ? placeholder : iso;
}

interface TriggerProps {
  /** Shown when nothing is selected. */
  placeholder: string;
  label: string;
  hasValue: boolean;
  disabled: boolean | undefined;
  onClear: (() => void) | undefined;
  className: string | undefined;
}

/**
 * The field-looking button that opens the calendar.
 *
 * A `<button>`, not an `<input>`: the value is chosen from the calendar,
 * and a text input that looks editable but is not is worse than a button
 * that looks like a field. Callers who want typed entry should use a
 * plain `<Input type="date">` — the browser's own picker is better than
 * anything worth reimplementing here.
 */
function PickerTrigger({
  placeholder,
  label,
  hasValue,
  disabled,
  onClear,
  className,
}: TriggerProps) {
  return (
    <div className={cn("relative inline-flex w-full items-center", className)}>
      <PopoverButton
        {...omitUndefined({ disabled })}
        className={cn(
          "input input-bordered flex w-full items-center gap-2 text-left font-sans text-prose",
          !hasValue && "text-subtle-foreground",
          "disabled:cursor-not-allowed disabled:opacity-50",
          hasValue && onClear !== undefined && "pr-9",
        )}
      >
        <CalendarDays size={14} strokeWidth={1.5} className="shrink-0 text-subtle-foreground" />
        <span className="truncate font-mono tabular-nums">{label}</span>
        <span className="sr-only">{placeholder}</span>
      </PopoverButton>
      {hasValue && onClear !== undefined && (
        // Outside the PopoverButton on purpose: nesting a button inside a
        // button is invalid HTML, and browsers resolve it by hoisting the
        // inner one out, which silently detaches the click handler.
        <button
          type="button"
          onClick={onClear}
          aria-label={`Clear ${placeholder}`}
          className="-translate-y-1/2 absolute top-1/2 right-2 text-subtle-foreground hover:text-foreground"
        >
          <X size={14} strokeWidth={1.5} />
        </button>
      )}
    </div>
  );
}

const PANEL_CLASS = cn(
  "z-50 rounded-md border border-edge bg-surface-2 shadow-[var(--shadow-popover)]",
  "[--anchor-gap:6px] focus:outline-none",
  "origin-top transition duration-100 ease-out data-closed:scale-95 data-closed:opacity-0",
);

export interface DatePickerProps {
  value: IsoDate | undefined;
  onValueChange: (value: IsoDate | undefined) => void;
  /** Shown when empty, and used as the control's accessible name. */
  placeholder?: string;
  /** Earliest selectable date, inclusive. */
  min?: IsoDate | undefined;
  /** Latest selectable date, inclusive. */
  max?: IsoDate | undefined;
  disabled?: boolean | undefined;
  /** Show an inline clear affordance once a date is picked. */
  clearable?: boolean;
  className?: string | undefined;
}

/** A single calendar date, chosen from a popover calendar. */
export function DatePicker({
  value,
  onValueChange,
  placeholder = "Pick a date",
  min,
  max,
  disabled,
  clearable = true,
  className,
}: DatePickerProps) {
  const selected = value === undefined ? undefined : fromIsoDate(value);

  return (
    <HeadlessPopover className={cn("relative", className)}>
      {({ close }) => (
        <>
          <PickerTrigger
            placeholder={placeholder}
            label={displayLabel(value, placeholder)}
            hasValue={value !== undefined && value !== ""}
            disabled={disabled}
            onClear={clearable ? () => onValueChange(undefined) : undefined}
            className={undefined}
          />
          <PopoverPanel anchor="bottom start" transition className={PANEL_CLASS}>
            <Calendar
              mode="single"
              {...omitUndefined({
                selected,
                defaultMonth: selected,
                startMonth: min === undefined ? undefined : fromIsoDate(min),
                endMonth: max === undefined ? undefined : fromIsoDate(max),
              })}
              onSelect={(next) => {
                onValueChange(next === undefined ? undefined : toIsoDate(next));
                // Closing on pick is right for a single date and wrong for
                // a range, where the first click is only half the answer.
                if (next !== undefined) close();
              }}
            />
          </PopoverPanel>
        </>
      )}
    </HeadlessPopover>
  );
}

/** Two ISO dates, inclusive at both ends. `undefined` at either end means open. */
export interface IsoDateRange {
  from?: IsoDate | undefined;
  to?: IsoDate | undefined;
}

export interface DateRangePickerProps {
  value: IsoDateRange | undefined;
  onValueChange: (value: IsoDateRange | undefined) => void;
  placeholder?: string;
  min?: IsoDate | undefined;
  max?: IsoDate | undefined;
  disabled?: boolean | undefined;
  clearable?: boolean;
  /** Months shown side by side. Two makes a cross-month range selectable
   * without navigating, which is the common case for "last 30 days". */
  numberOfMonths?: number;
  className?: string | undefined;
}

function rangeLabel(value: IsoDateRange | undefined, placeholder: string): string {
  if (value === undefined) return placeholder;
  const { from, to } = value;
  if (from === undefined && to === undefined) return placeholder;
  if (from !== undefined && to === undefined) return `${from} → …`;
  if (from === undefined && to !== undefined) return `… → ${to}`;
  return `${from} → ${to}`;
}

/**
 * A start and end date as one control.
 *
 * Exists because the alternative — two `<input type="date">` side by side
 * — is what applications actually ship, and it cannot express the one
 * thing a range needs: that the two ends constrain each other. Here the
 * second click is always after the first, the fill between them is
 * visible, and a half-finished range is representable (`from` set, `to`
 * open) rather than being a validation error.
 */
export function DateRangePicker({
  value,
  onValueChange,
  placeholder = "Pick a date range",
  min,
  max,
  disabled,
  clearable = true,
  numberOfMonths = 2,
  className,
}: DateRangePickerProps) {
  const selected: DateRange | undefined =
    value === undefined
      ? undefined
      : {
          from: value.from === undefined ? undefined : fromIsoDate(value.from),
          to: value.to === undefined ? undefined : fromIsoDate(value.to),
        };

  const hasValue = value !== undefined && (value.from !== undefined || value.to !== undefined);

  return (
    <HeadlessPopover className={cn("relative", className)}>
      <PickerTrigger
        placeholder={placeholder}
        label={rangeLabel(value, placeholder)}
        hasValue={hasValue}
        disabled={disabled}
        onClear={clearable ? () => onValueChange(undefined) : undefined}
        className={undefined}
      />
      <PopoverPanel anchor="bottom start" transition className={PANEL_CLASS}>
        <Calendar
          mode="range"
          numberOfMonths={numberOfMonths}
          {...omitUndefined({
            selected,
            defaultMonth: selected?.from,
            startMonth: min === undefined ? undefined : fromIsoDate(min),
            endMonth: max === undefined ? undefined : fromIsoDate(max),
          })}
          onSelect={(next) => {
            if (next === undefined) {
              onValueChange(undefined);
              return;
            }
            onValueChange({
              from: next.from === undefined ? undefined : toIsoDate(next.from),
              to: next.to === undefined ? undefined : toIsoDate(next.to),
            });
          }}
        />
      </PopoverPanel>
    </HeadlessPopover>
  );
}
