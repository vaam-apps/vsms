# Input primitives

Everything here imports from `@vaam-apps/ui`. Behaviour is Headless UI's
(focus, keyboard, ARIA); appearance is daisyUI plus this theme's tokens.
Merge your own classes with `cn()` — see the pitfalls page for what plain
concatenation does.

The recurring decision on this page is **bare control or wrapper**. The
library ships both for most of these, and the wrapper is not sugar: it is
the only version that associates the label, the hint and the error with
the control. A bare control plus a hand-written `<p>` is valid HTML that
reaches nobody.

## Button

Four variants and no more: `primary` (the achromatic inverse fill),
`secondary` (outline), `ghost`, `destructive`. There is deliberately **no
success or warning button** — those hues belong to the status system, and
a button that borrows one erodes the state language on every screen where
both appear.

| Prop | Type | Default |
|---|---|---|
| `variant` | `"primary" \| "secondary" \| "ghost" \| "destructive"` | `"primary"` |
| `size` | `"sm" \| "md" \| "icon"` | `"md"` |

Everything else is `<button>`'s own attributes, and the ref lands on the
element. `ButtonVariant` and `ButtonSize` are exported if you need to
thread one through your own component. Note that no `type` is set for
you, so a `Button` inside a `<form>` is a submit button until you say
`type="button"`.

`size="icon"` is **circular**, not square, and that is a convention the
whole library follows: a circle reads as "acts on the thing beside it", a
rounded rectangle as "this is a named action". An icon-only button has no
text, so it needs `aria-label` — nothing else will name it.

There is no `asChild`. A link that must look like a button reaches for
the class string directly, which keeps the anchor a real anchor and keeps
middle-click and "copy link address" working:

```tsx
import { Button, buttonVariants } from "@vaam-apps/ui";

<div className="flex items-center gap-2">
  <Button onClick={retryPayout}>Retry payout</Button>
  <Button variant="secondary" size="sm" onClick={copyReference}>
    Copy reference
  </Button>
  <Button variant="destructive" onClick={cancelPayout}>
    Cancel payout
  </Button>
  <Button size="icon" aria-label="Refresh this payout" onClick={refetch}>
    <RefreshCw size={14} strokeWidth={1.5} />
  </Button>
  <a href="/payouts/p_01J8" className={buttonVariants({ variant: "ghost", size: "sm" })}>
    Open in full
  </a>
</div>
```

## Input and Textarea

Thin wrappers over `<input>` and `<textarea>`: every native attribute
passes through, refs forward to the element, and both are `w-full` by
default. They add two things — this theme's type and field styling, and a
danger-tone border and text colour under `aria-invalid`.

That last point is why you should not set the error colour yourself. Wrap
the control in a `FormField` with an `error`, and the invalid styling
follows from the same attribute a screen reader is reading.

```tsx
<Input id="payout-msisdn" inputMode="tel" defaultValue="+237650000000" />
<Textarea id="payout-note" rows={3} placeholder="Why this payout was held" />
```

## Label

A standalone `<label>` for the cases `FormField` does not cover — a
control inside a table cell, a filter bar where the label sits elsewhere
in the layout. It takes `<label>`'s own props; `htmlFor` is what makes it
work, and a `Label` without one associates with nothing.

```tsx
<Label htmlFor="filter-provider">Provider</Label>
```

Inside a `FormField` you never write this yourself — the component
renders it for you from the `label` prop.

## FormField and FieldError

`FormField` is the labelled-control shape: label, optional hint, the
control, optional error. It exists because the same three elements were
hand-assembled at 80 call sites in the console this library was extracted
from, with the error paragraph inlined 35 times and several labels
carrying no `htmlFor` at all.

```tsx
interface FormFieldProps {
  label: ReactNode;
  htmlFor: string;              // required — see below
  hint?: ReactNode;             // rendered between the label and the control
  error?: string | undefined;   // undefined renders no error element at all
  control?: "field" | "group";  // default "field"
  className?: string | undefined;
  children: ReactNode;
}
```

**What it actually wires.** It clones its single child control and adds
`aria-describedby` pointing at whichever of the hint and error exist (in
that order, space-joined, merged with any `aria-describedby` the child
already had), plus `aria-invalid` when there is an error. So the operator
tabbing into a bad field hears the label, then the hint, then the message
— and the `Input` picks up its danger border from the same attribute.

**`htmlFor` is required on purpose.** A `Label` with no `htmlFor` breaks
click-to-focus and screen-reader association silently; making the prop
required turns that into a compile error instead.

**The cloning has real limits, and they fail quietly.**

- Only a single valid element child is cloned. Text, `null`, an array of
  children, **and a fragment** all fall through untouched — a fragment
  passes `isValidElement`, so `<FormField error>{<><Input /><Adornment /></>}</FormField>`
  renders an error nothing points at. Put the extra element inside a
  wrapping component that forwards its props, or wire the ids yourself.
- A control from outside this library only honours the cloned props if it
  forwards unknown props to a DOM node. `Input` and `Textarea` do.
  `Select`, `DatePicker` and `DateRangePicker` declare them explicitly and
  forward them — which they did not always do, and the failure was a hint
  rendered with an id that nothing referenced.
- **A hint cannot reach a `Select`.** Headless UI's `ListboxButton` builds
  its own `aria-describedby` from an internal description context and lets
  its own props win, so a value handed down is overwritten with
  `undefined` before it reaches the DOM. `aria-invalid` threads fine.
  There is a test pinning exactly what each control emits, so this will
  announce itself if it ever changes. Until then: put anything a `Select`
  user must read into the label, not the hint.

**`control="group"`** is for `RadioGroup` and `ChipSelect`. HTML's `for`
only associates with labelable elements, so pointing it at a `<div>` or a
`<fieldset>` is invalid *and* dangling — which is what one migration
shipped, four labels pointing at ids no element carried. In group mode no
`for` is emitted; the label gets a derived id and the group points back at
it with `aria-labelledby`. See the `RadioGroup`/`ChipSelect` example
below for the pairing.

**Hints are a person's sentence, and render italic.** The line the
library draws: if the string could be a template filling in a value the
system already has — a count, a ratio, an id — it is an emitted fact and
stays sans ("4 of 9 attempts delivered"). If a person decided a
constraint and wrote it out ("Three to eleven characters."), it is a
hint.

`FieldError` is the same red caption rendered standalone, for a form-level
message above the submit button or an error attached to a group rather
than one input. It is `role="alert"`, so it announces when it appears;
inside a `FormField` it also carries the id the control's
`aria-describedby` names, which is what makes a pre-existing error
readable to someone tabbing back in later.

```tsx
import { FieldError, FormField, Input } from "@vaam-apps/ui";

<form onSubmit={submit}>
  <FormField
    label="Sender ID"
    htmlFor="sender-id"
    hint="Three to eleven characters."
    error={errors.senderId?.message}
  >
    <Input id="sender-id" {...form.register("senderId")} />
  </FormField>

  {submitError !== undefined && <FieldError>{submitError}</FieldError>}
  <Button type="submit">Submit for review</Button>
</form>
```

## Select, SelectTrigger, SelectValue, SelectContent, SelectGroup, SelectItem

A listbox, for a vocabulary too long to show at once. For a handful of
options prefer `RadioGroup` or `ChipSelect` — a select costs one click to
discover the choices and another to pick one.

The composition is the Radix shape kept deliberately intact:

- **`Select`** owns the value. `value` / `defaultValue` / `onValueChange`
  (all `string`), plus `disabled` and `aria-invalid`. Controlled when
  `value` is passed, uncontrolled from `defaultValue` otherwise. Note the
  explicit `| undefined` on the optional props: this package compiles
  under `exactOptionalPropertyTypes`, so the ordinary
  `useState<string>()` → `value={v}` pattern would otherwise be a type
  error.
- **`SelectTrigger`** is the button. Takes `id`, `className`, and
  `aria-label` / `aria-labelledby` for a select used outside a
  `FormField` — the trigger only ever renders the selected value, so
  nothing else can name it.
- **`SelectValue`** renders the selected item's *children* (the label),
  not the raw value, falling back to the value if no item matches. Its
  `placeholder` shows while nothing is selected.
- **`SelectContent`** is the dropdown. It renders **inline, not
  portalled** — a correctness fix, not a preference: Headless UI's
  portalled options land outside a vaul drawer's focus trap, where the
  enter transition stalls and the listbox is measurably never usable
  (`opacity: 0`, `pointer-events: none`, a collapsed rect). Every select
  inside a drawer was broken that way until it was rendered inline.
- **`SelectItem`** takes a `value` and its label as children. Labels clamp
  to two lines.
- **`SelectGroup`** is semantic grouping only — a `contents` fieldset with
  no visual treatment of its own. Nothing in the console uses it today; it
  exists for API parity.

```tsx
import {
  FormField, Select, SelectContent, SelectItem, SelectTrigger, SelectValue,
} from "@vaam-apps/ui";

const [provider, setProvider] = useState<string>();

<FormField label="Provider" htmlFor="route-provider">
  <Select value={provider} onValueChange={setProvider}>
    <SelectTrigger id="route-provider">
      <SelectValue placeholder="Any provider" />
    </SelectTrigger>
    <SelectContent>
      <SelectItem value="orange_cm">Orange Cameroon</SelectItem>
      <SelectItem value="mtn_agg">MTN (aggregator)</SelectItem>
      <SelectItem value="twilio">Twilio</SelectItem>
    </SelectContent>
  </Select>
</FormField>
```

## Checkbox / CheckboxField, Switch / SwitchField

**Which control.** A checkbox is a value a Save button will commit; a
switch promises the change *has already happened*. Pairing a switch with a
Save button tells the operator two contradictory things and leaves them
unsure whether the toggle landed. Use a switch only where the write fires
on change and the UI can report the result.

**Bare or `*Field`.** The bare control is for a row whose surface already
carries the meaning — a select-all box in a table header, a toggle in a
column. It then needs `aria-label` or `aria-labelledby`, because nothing
visible names it. The `*Field` version renders a clickable label (and an
optional one-line `description`) beside the control, which is the settings
row. These are separate from `FormField`, which stacks a label *above* a
control: a checkbox's label belongs on the same line and must itself be a
click target.

```ts
interface CheckboxProps {
  checked: boolean;
  onCheckedChange?: ((checked: boolean) => void) | undefined; // omit for display-only
  indeterminate?: boolean;
  disabled?: boolean | undefined;
  "aria-label"?: string | undefined;
  "aria-labelledby"?: string | undefined;
  className?: string | undefined;
}
```

`SwitchProps` is the same minus `indeterminate`, and there
`onCheckedChange` is **required**: `Checkbox` allows it to be omitted for
a display-only box inside a row whose whole surface handles the click,
and a switch has no equivalent case.

`CheckboxFieldProps` / `SwitchFieldProps` drop the two aria props and add
`label: ReactNode` and `description?: ReactNode`. Descriptions clamp to two
lines: a settings list whose rows are all 44px except the one with a
paragraph in it reads as broken rather than as informative.

`indeterminate` renders a dash and reports `aria-checked="mixed"` — the
parent-row state where the children disagree. `checked` is still what a
click toggles from.

```tsx
<CheckboxField
  checked={maskRecipient}
  onCheckedChange={setMaskRecipient}
  label="Mask the recipient in webhook payloads"
  description="The masked form is baked in when the attempt row is written, not at delivery."
/>

<SwitchField
  checked={livePolling}
  onCheckedChange={setLivePolling}
  label="Live updates"
  description="Poll for new rows while this screen is open."
/>

{/* Bare, inside a table header cell — the column already says what it is. */}
<Checkbox
  checked={allSelected}
  indeterminate={someSelected && !allSelected}
  onCheckedChange={toggleAll}
  aria-label="Select every message on this page"
/>
```

## RadioGroup and ChipSelect

Two controls over a **small, fixed vocabulary**, shown in full rather than
hidden behind a trigger. `RadioGroup` picks one; `ChipSelect` picks any
number. Both render inline with no portal and no transition, so neither
can hit the focus-trap failure that made `Select` unusable inside a
drawer — for a bounded vocabulary in a drawer, these are the *safer*
control, not merely the friendlier one.

```ts
interface RadioGroupProps<T extends string> {
  value: T | undefined;
  onValueChange: (value: T) => void;
  options: readonly RadioGroupOption<T>[];   // { value, label, description? }
  disabled?: boolean | undefined;
  "aria-label"?: string | undefined;
  "aria-labelledby"?: string | undefined;
  className?: string | undefined;
}

interface ChipSelectProps<T extends string> {
  value: readonly T[];
  onValueChange: (value: T[]) => void;
  options: readonly ChipOption<T>[];         // { value, label, description? }
  disabled?: boolean | undefined;
  "aria-label"?: string | undefined;
  "aria-labelledby"?: string | undefined;
  className?: string | undefined;
}
```

Both option types take a **`description`**, and both wire it as a real
description rather than as text inside the option. That matters: the label
and the description both render inside the `role="radio"` element, so
content-based naming used to concatenate them — the option was named
"ApproveThe provider accepted it.", and the whole sentence was re-read on
every arrow press. axe reports nothing, because the radio has a role and a
non-empty name; it is just the wrong text. The fix is a Headless UI field
per option, which registers the description through context while keeping
the whole card clickable. Write the description as a description; do not
fold it into the label.

Selection is spelled achromatically — a filled `primary` glyph on a
`surface-3` row — not with a status hue. Status hues answer "what did the
system decide"; these answer "what did you pick". An earlier version used
the success tokens, and since `success` is a quiet hue whose background
and border are declared `transparent`, the chosen option rendered with no
fill and no outline, reading as *less* present than the ones nobody had
picked. The keyboard focus ring was invisible for the same reason.

`ChipSelect` renders its labels in the mono face — the motivating case is
OAuth scopes, where the label *is* the literal token the API expects.

```tsx
import { ChipSelect, FormField, RadioGroup, groupLabelId } from "@vaam-apps/ui";

<FormField
  label="Message class"
  htmlFor="message-class"
  hint="Governs which senders may deliver this message."
  control="group"
>
  <ChipSelect
    aria-labelledby={groupLabelId("message-class")}
    value={classes}
    onValueChange={setClasses}
    options={[
      { value: "otp", label: "OTP" },
      { value: "transactional", label: "Transactional" },
      {
        value: "marketing",
        label: "Marketing",
        description: "Requires the recipient's prior consent.",
      },
    ]}
  />
</FormField>

<RadioGroup
  aria-label="Registration decision"
  value={decision}
  onValueChange={setDecision}
  options={[
    { value: "approved", label: "Approve", description: "The provider accepted it." },
    { value: "rejected", label: "Reject", description: "Needs a reason." },
  ]}
/>
```

## Calendar, DatePicker, DateRangePicker, and the ISO helpers

### The boundary rule

The pickers exchange **`YYYY-MM-DD` strings**, never date objects. The
type is `IsoDate` (an alias for `string`), and a range is `IsoDateRange`
(`{ from?: IsoDate; to?: IsoDate }`, inclusive at both ends, either end
`undefined` meaning open). A timestamp is an *instant*, and an instant
rendered in another zone is a different calendar day — which is how a
filter for "today" quietly returns yesterday's rows for anyone west of the
server. Date objects are confined to the picker internals.

`toIsoDate` and `fromIsoDate` are the two crossings, and both avoid UTC on
purpose:

```ts
function toIsoDate(date: Date): IsoDate;
function fromIsoDate(iso: IsoDate): Date | undefined;
```

- `toIsoDate` reads the **local** year, month and day off the object — it
  never formats through UTC, which would shift the day for half the world.
- `fromIsoDate` parses `YYYY-MM-DD` to **local midnight**, and returns
  `undefined` for anything that does not match that exact shape.
  Deliberately not the built-in date parse of an ISO string, which the
  spec treats as UTC midnight — in any negative-offset zone that is the
  previous day, so a calendar built on it highlights the wrong cell.

You need these only when talking to something that works in date objects
(a chart axis, a bare `Calendar`). Application code should hold the ISO
string and pass it straight to the API.

### DatePicker and DateRangePicker

```ts
interface DatePickerProps {
  value: IsoDate | undefined;
  onValueChange: (value: IsoDate | undefined) => void;
  placeholder?: string;              // default "Pick a date"
  min?: IsoDate | undefined;         // earliest selectable, inclusive
  max?: IsoDate | undefined;         // latest selectable, inclusive
  disabled?: boolean | undefined;
  clearable?: boolean;               // default true
  "aria-describedby"?: string | undefined;
  "aria-invalid"?: boolean | undefined;
  className?: string | undefined;
}
```

`DateRangePickerProps` is the same with `value: IsoDateRange | undefined`,
a default placeholder of "Pick a date range", and `numberOfMonths`
(default `2`, so "last 30 days" is selectable without navigating).

`min` / `max` clamp navigation as well as selection — the chevrons stop
rather than letting the operator wander into a month with nothing
selectable. `clearable` adds an inline clear control once a date is
picked; it sits outside the trigger button, because a button nested in a
button is invalid HTML that browsers resolve by hoisting the inner one
out, silently detaching its handler.

`placeholder` doubles as the accessible name: it is rendered into the
trigger as visually hidden text, so a picker outside a `FormField` is
still named. The single picker closes on pick; the range picker stays
open, because the first click is only half the answer, and a half-finished
range (`from` set, `to` open) is representable rather than a validation
error.

The trigger is a button, not an editable field — the value comes from the
calendar, and a text input that looks editable but is not is worse than a
button that looks like a field. If you want typed entry, use a plain
`<Input type="date">` and let the browser's own picker do it.

```tsx
import {
  DatePicker, DateRangePicker, FormField, type IsoDate, type IsoDateRange,
} from "@vaam-apps/ui";

const [settledOn, setSettledOn] = useState<IsoDate | undefined>();
const [attemptWindow, setAttemptWindow] = useState<IsoDateRange | undefined>({
  from: "2026-09-03",
  to: "2026-09-14",
});

<FormField label="Settlement date" htmlFor="settled-on">
  <DatePicker
    value={settledOn}
    onValueChange={setSettledOn}
    min="2026-09-01"
    max="2026-09-30"
    placeholder="A day in September"
  />
</FormField>

<DateRangePicker
  value={attemptWindow}
  onValueChange={setAttemptWindow}
  placeholder="Attempt window"
/>
```

### Calendar

The bare month grid, for a different shell than the popover — a filter
panel, a sidebar, anything that wants the calendar always visible. It is
`react-day-picker` with every element's class supplied from this system's
tokens; its own stylesheet is deliberately **not** imported, so there is
no second theme in the page to drift from `theme.css`. `CalendarProps` is
that library's own props plus `className`, passed straight through, which
has one consequence worth knowing:

**`selected` without `onSelect` is not a binding.** Given only `selected`,
react-day-picker manages selection internally and treats your prop as an
initial value — so the component looks controlled and silently diverges
from its own state the first time someone clicks a day. Pass both, or
neither. `Calendar` works in date objects, so this is where `fromIsoDate`
and `toIsoDate` earn their keep.

```tsx
import { Calendar, fromIsoDate, toIsoDate } from "@vaam-apps/ui";

<Calendar
  mode="single"
  selected={settledOn === undefined ? undefined : fromIsoDate(settledOn)}
  onSelect={(day) => setSettledOn(day === undefined ? undefined : toIsoDate(day))}
  defaultMonth={new Date(2026, 8, 1)}
/>
```
