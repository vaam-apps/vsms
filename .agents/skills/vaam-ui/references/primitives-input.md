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

| Prop      | Type                                                   | Default     |
| --------- | ------------------------------------------------------ | ----------- |
| `variant` | `"primary" \| "secondary" \| "ghost" \| "destructive"` | `"primary"` |
| `size`    | `"sm" \| "md" \| "icon"`                               | `"md"`      |

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
  `undefined` before it reaches the DOM. A searchable `Select`'s trigger
  (Headless UI's combobox button) does the same. `aria-invalid` threads
  fine.
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

## Select and its parts

`Select`, `SelectTrigger`, `SelectValue`, `SelectContent`, `SelectDropdown`,
`SelectModal`, `SelectItem`, `SelectGroup`, `SelectSearch`, `SelectEmpty`,
`SelectClose`, `SelectModalHandle`.

A picker, for a vocabulary too long to show at once. For a handful of
options prefer `RadioGroup` or `ChipSelect` — a select costs one click to
discover the choices and another to pick one.

It is a **compound component**: `Select` is the wrapper that owns the
value, and everything visible is a part you nest inside it. You describe
*what* you want; the library decides *how* — which engine, which
presentation at which width. Adding a `<SelectSearch />` is the whole
switch from a plain picker to a searchable one.

### The value and the trigger

- **`Select`** owns the value. `value` / `defaultValue` / `onValueChange`
  (all `string`), plus `disabled` and `aria-invalid`. Controlled when
  `value` is passed, uncontrolled from `defaultValue` otherwise. Note the
  explicit `| undefined` on the optional props: this package compiles
  under `exactOptionalPropertyTypes`, so the ordinary
  `useState<string>()` → `value={v}` pattern would otherwise be a type
  error.
- **`SelectTrigger`** is the button. Takes `id`, `className`, and
  `aria-label` for a select used outside a `FormField` — the trigger only
  ever renders the selected value, so nothing else can name it. Use
  `aria-label`, not `aria-labelledby`: the latter is accepted but does not
  currently reach the rendered button (Headless UI's own label wiring
  overrides it), so a select named that way is named by its own text —
  the current value or the placeholder — instead of its label.
- **`SelectValue`** renders the selected item's *children* (the label),
  not the raw value. Its `placeholder` shows while nothing is selected.
  The label is found by walking `Select`'s children, so write
  `SelectItem`s directly rather than from inside your own wrapper
  component if the label differs from the value. An item's label is also
  remembered once the item has rendered, so the name stays when your own
  search results (`filter={false}`) no longer include the chosen item;
  only a value no item has ever shown falls back to the raw value.

### The container — pick one

- **`SelectContent`** (use this by default) — a dropdown under the
  trigger from 640px up; **below 640px an M3 modal bottom sheet** (pinned
  to the bottom edge, full width, 28px top corners, a scrim, capped at
  85dvh and scrolling, a drag handle that dismisses past 56px or on a
  flick, 56px rows with the current value as a filled row with 16px
  corners). With a `SelectSearch` it is instead M3's **docked search
  view** from 640px up and its **full-screen search view** below. Chosen
  by CSS media query, so it server-renders correctly.
- **`SelectDropdown`** — the dropdown (or docked search view) at *every*
  width, phones included. The opt-out, for a two- or three-option select
  inline in a dense form row. Most selects should not use it.
- **`SelectModal`** — the sheet (or full-screen search view) at every
  width. From 640px up, both are capped at 640px and centred; the
  searchable one is then the full window height, with the sheet's 28px top
  corners and scrim.

All three render **inline, not portalled** — a correctness fix, not a
preference: Headless UI's portalled options land outside a vaul drawer's
focus trap, where they never become usable. So they all work inside a
`Drawer`. The dropdown (and the docked search view) is nonetheless
`position: fixed`, placed under its trigger by Floating UI, so a scrolling
or clipping ancestor — a `Dialog`'s body, a drawer's, a card with
`overflow-hidden` — no longer cuts it off. The exception is a container
that clips *and* has a `transform`, `filter`, `backdrop-filter`,
`contain` or `will-change: transform`: that one still clips it, so do not
put a `Select` in one. When its trigger scrolls out of its container's
view, the dropdown fades out and ignores the pointer until the trigger
scrolls back; while faded it still answers the keyboard, and Escape
closes it. It matches the trigger's width,
shrinks to the room below it, and opens *above* the trigger only when it
does not fit below and less than 200px is left there (a short list that
fits never flips); with too little room either side it keeps the better
side, shorter. Near the right edge of the window a docked search view
aligns to its trigger's right edge instead of its left. Its placement is
the library's: do not pass `top-*`, `left-*`, `inset-*` or `mt-*` in
`className` (they replace or add to it), and know that a `max-h-*` there
replaces the cap that keeps it inside the window. Do not put a `Select` inside something that re-anchors
`position: fixed` children at phone width (a `transform`ed or
`will-change`d ancestor that is *not* pinned to the bottom edge — the
sheet anchors to that ancestor instead of the screen). A width you pass
to `SelectContent`'s `className` (`w-64`) applies to the dropdown only;
the phone sheet stays full-width unless you pass a `max-sm:` width.
`SelectModal` inside a desktop side drawer is laid out against the
drawer's panel, not the window — measured at 1440px, a 640px modal
centred in `MoreDetailDrawer`'s 680px panel rather than on the screen.

### The options

- **`SelectItem`** takes a `value` and its label as children. Labels clamp
  to two lines. Any string is a valid `value`, `""` included (an "Any
  country" row): picked, the trigger shows its label. Without a
  `<SelectItem value="">`, `value=""` on `Select` still means "nothing
  picked" and shows the placeholder. `textValue` is what search matches against — pass it
  when the children are not plain text, or to be findable by a word not
  shown. It *replaces* the children's text rather than adding to it, so
  include the label: `textValue="Cameroon CM"` makes "CM" find Cameroon,
  where `textValue="CM"` would stop "Cameroon" finding it.
- **`SelectGroup`** is semantic grouping only — a `contents` fieldset with
  no visual treatment of its own.

### Search — a searchable select

- **`SelectSearch`** — an M3 search field in the popup's header (write it
  first among the container's children; it is lifted into the header
  wherever you put it, but it must be a direct part, not inside your own
  wrapper component). It makes the select a WAI-ARIA combobox: focus stays
  in the field while the arrow keys move through the options, Enter picks,
  Escape closes. Enter with nothing to pick does nothing (the view and the
  text stay). Tab leaves the field and closes the popup *without* picking
  anything. Matching is case- and accent-insensitive ("cote" finds "Côte
  d’Ivoire"). `placeholder`; `aria-label` (default "Search" plus the
  select's own name — "Search Country" inside a `FormField` labelled
  Country); `clearLabel`, the clear button's name (default "Clear
  search"); `onQueryChange(query)` (called as the text changes, and with
  `""` on close if anything was typed — not on the close of a popup nobody
  searched, so a per-query fetch does not refetch the full list on every
  open and close); and `filter` — pass `filter={false}` when you filter or
  fetch the items yourself from `onQueryChange`; every item you render is
  then shown. Adding or removing the `SelectSearch` swaps the engine and
  remounts the trigger and the popup, so change it only while the select
  is closed.
- **`SelectEmpty`** — shown when no option is shown. Every searchable
  popup renders "No match" on its own; write a `SelectEmpty` to say more
  ("No country matches", or "Searching…" while your fetch is in flight).
  It is plain text, not a live region itself: the popup keeps one polite
  `role="status"` mounted while it is open and puts this text into it
  when the list empties, which is what a screen reader reliably
  announces.

### Chrome — public parts with defaults, so you rarely write them

- **`SelectClose`** — closes the popup. The full-screen search view leads
  with one (a back arrow, named "Back") unless you write your own as a
  direct part of the container — one wrapped in your own element does not
  replace it. Add one anywhere else with any children
  (`<SelectClose>Done</SelectClose>`); with children, they are its name
  and `aria-label` is ignored. Where it lands decides what it is. Written
  as a direct part of a searchable select's container, it is lifted into
  the header beside the field and is a real button, named by its children
  or, with none, by `aria-label` (default "Back").
  Anywhere inside the list — every `SelectClose` in a plain select, or one
  you wrap in your own element in a searchable one — it is a pointer-only
  affordance (`aria-hidden`, out of the tab order), because a listbox may
  only expose options; keyboard users have Escape.
- **`SelectModalHandle`** — the sheet's M3 drag handle. Every sheet
  renders one at its top unless you place your own; there is none in a
  dropdown or a search view.

**Behaviour you get for free:** Escape, a tap on the dimmed page, the back
arrow, a drag of the handle, or picking an option closes it and returns
focus to the trigger. Inside a drawer, any of those closes the `Select`
alone; the next Escape closes the drawer. While a searchable popup is
open the rest of the page is inert and does not scroll, inside a `Dialog`
or `Drawer` too, and both are restored on close — including when your
`onValueChange` closes the dialog around it. `SideNav`'s bottom toolbar
hides itself while any `Select` is open below 640px, and its vertical
toolbar while a `SelectModal` is open.

**Known limit.** The full-screen search view is `100dvh` tall, and
neither iOS Safari nor Android Chrome (by default) shrinks `dvh` for the
on-screen keyboard — it overlays the page — so the last results can sit
under the keyboard until it is dismissed.

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

Searchable — the same parts plus a `SelectSearch`:

```tsx
import {
  FormField, Select, SelectContent, SelectEmpty, SelectItem, SelectSearch,
  SelectTrigger, SelectValue,
} from "@vaam-apps/ui";

<FormField label="Country" htmlFor="sender-country">
  <Select value={country} onValueChange={setCountry}>
    <SelectTrigger id="sender-country">
      <SelectValue placeholder="Choose a country" />
    </SelectTrigger>
    <SelectContent>
      <SelectSearch placeholder="Search countries" />
      {countries.map((c) => (
        <SelectItem key={c.code} value={c.code} textValue={`${c.name} ${c.code}`}>
          {c.name}
        </SelectItem>
      ))}
      <SelectEmpty>No country matches</SelectEmpty>
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

### DatePicker and DateRangePicker, and their parts

`DatePicker`, `DateRangePicker`, `DatePickerTrigger`, `DatePickerValue`,
`DatePickerClear`, `DatePickerContent`, `DatePickerDropdown`,
`DatePickerModal`, `DatePickerTitle`, `DatePickerCancel`,
`DatePickerConfirm`, `DatePickerClose`.

A **compound component**, shaped like `Select`: the root (`DatePicker` for
one date, `DateRangePicker` for a range) owns the value, and every visible
piece is a part nested inside it. The same parts serve both roots.

```ts
interface DatePickerProps {
  value: IsoDate | undefined;
  onValueChange: (value: IsoDate | undefined) => void;
  min?: IsoDate | undefined;         // earliest selectable, inclusive
  max?: IsoDate | undefined;         // latest selectable, inclusive
  disabled?: boolean | undefined;
  "aria-describedby"?: string | undefined;
  "aria-invalid"?: boolean | undefined;
  children: ReactNode;
}
```

`DateRangePickerProps` is the same with `value: IsoDateRange | undefined`.
A half-finished range (`from` set, `to` open) is a value, not a validation
error. `aria-describedby` and `aria-invalid` are what `FormField` hands
its child; the root passes both on to the trigger.

**A pick is staged, then confirmed, at every width.** Tapping a day only
moves the pick. A range follows M3's rule: the first tap sets the start, a
tap on or after the start sets the end, and a tap before the start or on a
whole range starts a new range. `onValueChange` fires once, when the operator presses OK
(Save for a range). Cancel, Escape, a press outside the picker and the
full-screen picker's close icon all throw the pick away. OK is disabled
until there is a date to commit; a range needs only its start, and
committing a start alone gives an open-ended range (`{ from, to:
undefined }`). Focus goes
back to the trigger on close.

**A press outside only closes the picker.** It does not also act on what
it lands on: the page is inert while the picker is open, and the rest of
that press (its click included) is spent on closing. So a button beside
an open picker takes two presses: one to close the picker, one to press
it. Inside a `Dialog`, a `Drawer` or your own Radix dialog (its overlay
wrapping the content or not), the same press, by mouse or touch and however
long it is held, closes the picker and leaves the dialog open.

#### The parts

- **`DatePickerTrigger`** is the field-looking button. It takes `id` (the
  `htmlFor` of a `FormField`), `className`, `children` and `aria-label`.
  It is named by its label: a `FormField` label, or `aria-label` for a
  picker used outside one. The date it shows is added to its description,
  so a screen reader hears "Send on, button, 2026-09-11". With no label at
  all, it is named by its own text.
- **`DatePickerValue`** is the shown value: `YYYY-MM-DD`, or
  `from → to` for a range, in mono. While the value is empty it shows its
  `placeholder`.
- **`DatePickerClear`** empties the value at once, without opening the
  picker and without OK. Write it inside the trigger. It renders beside
  the button, over the field's right end, and only while there is a value
  and the picker is enabled. Its `aria-label` defaults to "Clear the
  date", or "Clear the dates" for a range. Leave it out for a date that
  must always be set.
- **A container, pick one.** Each takes `className` and, as `children`,
  any chrome parts you want to relabel.
  - **`DatePickerContent`** (the default) is M3's docked picker from
    640px up: a 360px panel under the trigger. Below 640px a single date
    opens M3's modal date picker: 360px wide (the window less 32px on a
    narrower phone) and centred over a scrim,
    with 28px corners and a 120px header that shows the pick as its
    headline. A range below 640px opens M3's full-screen range picker. It
    has a bar with a close icon and Save, a 128px header, and one pinned
    weekday row over the months, stacked vertically to scroll through:
    12 months either side of the range's start (or today), narrowed by
    `min`/`max`. The presentation is chosen by CSS media query, so it
    server-renders correctly.
  - **`DatePickerDropdown`** is docked at every width, phones included.
    It's the opt-out for a picker inline in a dense row.
  - **`DatePickerModal`** is the modal at every width, centred over a
    scrim on a wide window (a range shows two months there). Below 640px
    it is the same modal or full-screen picker as `DatePickerContent`.
- **The chrome** is public parts with defaults, so you rarely write them.
  Write one inside the container to relabel it:
  - `DatePickerTitle` defaults to "Select date" or "Select dates". It
    always names the picker. It is *shown* only where there is a header:
    a single date's modal, and the full-screen range picker below 640px.
    The docked picker, and a range's `DatePickerModal` from 640px up (two
    months, no header), show no title.
  - `DatePickerCancel` defaults to "Cancel".
  - `DatePickerConfirm` defaults to "OK", or "Save" for a range.
  - `DatePickerClose` is the full-screen range picker's close icon, at the
    start of its bar; it throws the pick away, as Cancel does elsewhere.
    Named "Close" by default. Give it an `aria-label` to rename it;
    `children` with an `aria-label` to replace the icon with another (a
    48px icon button named by the label); or `children` alone for a text
    button named by its text. Only the full-screen picker has a bar, so
    anywhere else a written one is not shown.

A docked range shows two months side by side from 768px up, and stacked
between 640px and 768px.

All three containers render **inline, not portalled**, so they work
inside a `Drawer`. Escape inside a drawer closes the picker, not the
drawer. The docked panel is `position: fixed`, placed under its trigger
by Floating UI exactly as `SelectDropdown` is, with the same caveat: a
container that clips *and* has a `transform`, `filter`,
`backdrop-filter`, `contain` or `will-change: transform` still clips it.
While any picker is open the rest of the page is inert and does not
scroll.

`min` / `max` clamp navigation as well as selection. The arrows stop,
rather than letting the operator wander into a month with nothing
selectable. Every day outside the bounds is disabled.

The trigger is a button, not an editable field — the value comes from the
calendar, and a text input that looks editable but is not is worse than a
button that looks like a field. If you want typed entry, use a plain
`<Input type="date">` and let the browser's own picker do it.

```tsx
import {
  DatePicker, DatePickerClear, DatePickerContent, DatePickerTrigger,
  DatePickerValue, DateRangePicker, FormField, type IsoDate, type IsoDateRange,
} from "@vaam-apps/ui";

const [settledOn, setSettledOn] = useState<IsoDate | undefined>();
const [attemptWindow, setAttemptWindow] = useState<IsoDateRange | undefined>({
  from: "2026-09-03",
  to: "2026-09-14",
});

<FormField label="Settlement date" htmlFor="settled-on">
  <DatePicker value={settledOn} onValueChange={setSettledOn} min="2026-09-01" max="2026-09-30">
    <DatePickerTrigger id="settled-on">
      <DatePickerValue placeholder="A day in September" />
      <DatePickerClear />
    </DatePickerTrigger>
    <DatePickerContent />
  </DatePicker>
</FormField>

<DateRangePicker value={attemptWindow} onValueChange={setAttemptWindow}>
  <DatePickerTrigger aria-label="Attempt window">
    <DatePickerValue placeholder="Any time" />
    <DatePickerClear />
  </DatePickerTrigger>
  <DatePickerContent />
</DateRangePicker>
```

#### Migrating from the single-component picker (0.2.x)

The old `<DatePicker value onValueChange placeholder />` rendered
everything itself. It is gone; there is no compatibility shim.

- **Nest the parts.** At minimum a `DatePickerTrigger` holding a
  `DatePickerValue`, and a `DatePickerContent`.
- **`placeholder`** moves to `<DatePickerValue placeholder>`. It no longer
  names the trigger: the old picker repeated it in visually hidden text,
  and an empty trigger was announced twice ("Created between Created
  between"). Outside a `FormField`, give the trigger an `aria-label`.
- **`clearable`** (default `true`) is replaced by writing
  `<DatePickerClear />` in the trigger. **The default flipped:** a picker
  with no `DatePickerClear` has no clear button. Add one wherever the old
  picker left `clearable` at its default.
- **The clear button's name changed.** It was `Clear ${placeholder}`
  ("Clear Created between"); it is now "Clear the date" / "Clear the
  dates", or its own `aria-label`. Tests that query it by name need the new
  one.
- **`className`** moves to `DatePickerTrigger`, or to the container for
  the picker's own panel.
- **`numberOfMonths`** on `DateRangePicker` is gone: two months docked,
  stacked in the full-screen picker.
- **A tap on a whole range starts a new one.** The old picker used
  react-day-picker's rule, which extended the range from its start, so a
  new start needed Clear first. Its first tap was also a one-day range
  (`from` = `to`); it is now a start with an open end until the second tap.
- **`onValueChange` fires on OK or Save only.** The old single picker
  committed and closed on the first click, and the old range picker
  committed each click while it stayed open. Code that filtered live as
  the operator clicked now filters once, on confirm.
- **Inside a `Drawer` it now works.** The old calendar was portalled out
  of the drawer, where no day could be clicked.

### Calendar

The bare month grid, for a different shell than the pickers — a filter
panel, a sidebar, anything that wants the calendar always visible. It
has M3's date picker geometry: 40px round days in 48px cells, a 56px
month row with the arrows at its end, the range as a band running between
two filled ends, and today as a ring. Unlike the pickers, it shows the
neighbouring months' days by default (`showOutsideDays`). It is
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
