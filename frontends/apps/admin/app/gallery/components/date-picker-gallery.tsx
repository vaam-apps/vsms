"use client";

// Route-local (R6). Added with the `react-day-picker` primitives so the
// gallery keeps its own stated contract — "it imports and exercises every
// `@vaam-apps/ui` export, so it doubles as the console's own visual-QA
// surface. A gallery that silently drops an export is a QA surface with a
// blind spot."

import {
  Calendar,
  DatePicker,
  DateRangePicker,
  type IsoDate,
  type IsoDateRange,
} from "@vaam-apps/ui";
import { useState } from "react";
import { GallerySwatch } from "./gallery-swatch";
import { Section } from "./section";

export function DatePickerGallery() {
  const [single, setSingle] = useState<IsoDate | undefined>("2026-09-11");
  const [range, setRange] = useState<IsoDateRange | undefined>({
    from: "2026-09-01",
    to: "2026-09-11",
  });
  const [bounded, setBounded] = useState<IsoDate | undefined>();
  // Controlled on purpose. `Calendar` passes props straight through to
  // react-day-picker, which manages selection ITSELF when `onSelect` is
  // omitted — so `selected` alone is an initial value, not a binding, and
  // the demo would silently drift away from what this file says it shows
  // the first time anyone clicked a day. Found by clicking one.
  const [bare, setBare] = useState<Date | undefined>(new Date(2026, 8, 11));

  return (
    <Section
      title="Date and date-range pickers"
      description="react-day-picker, themed from this system's own tokens rather than its stylesheet. Values cross the boundary as YYYY-MM-DD strings, never Date objects — an instant rendered in another zone is a different calendar day, which is how a filter for 'today' quietly returns yesterday's rows."
    >
      <div className="flex flex-col gap-4">
        <GallerySwatch label="DatePicker — closes on pick, clearable once set">
          <div className="flex flex-wrap items-start gap-4">
            <div className="w-64">
              <DatePicker value={single} onValueChange={setSingle} />
            </div>
            <p className="font-mono text-caption text-subtle-foreground">
              value: {single ?? "undefined"}
            </p>
          </div>
        </GallerySwatch>
        <GallerySwatch label="DateRangePicker — two months side by side, stays open between the two clicks, and can hold a half-finished range">
          <div className="flex flex-wrap items-start gap-4">
            <div className="w-80">
              <DateRangePicker value={range} onValueChange={setRange} />
            </div>
            <p className="font-mono text-caption text-subtle-foreground">
              value:{" "}
              {range === undefined ? "undefined" : `${range.from ?? "…"} → ${range.to ?? "…"}`}
            </p>
          </div>
        </GallerySwatch>
        <GallerySwatch label="Bounded — min/max clamp navigation as well as selection (September 2026 only)">
          <div className="w-64">
            <DatePicker
              value={bounded}
              onValueChange={setBounded}
              min="2026-09-01"
              max="2026-09-30"
              placeholder="Pick a day in September"
            />
          </div>
        </GallerySwatch>
        <GallerySwatch label="Calendar — the bare themed DayPicker, for a different shell. Controlled: `selected` without `onSelect` leaves react-day-picker managing its own selection.">
          <div className="inline-block rounded-md border border-edge bg-surface-2">
            <Calendar
              mode="single"
              selected={bare}
              onSelect={setBare}
              defaultMonth={new Date(2026, 8, 1)}
            />
          </div>
        </GallerySwatch>
      </div>
    </Section>
  );
}
