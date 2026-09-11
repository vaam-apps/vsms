# @vaam-apps/ui

A dark-first React component library for **operator consoles** — the
screens a person uses to find out why one record out of a hundred
thousand did not do what it was supposed to.

- **daisyUI** for styling, so component classes stay short and themeable.
- **Headless UI** for behaviour, so focus, keyboard and ARIA are not
  reimplemented per component.
- **A parameterised status system**, so one glyph vocabulary serves every
  state machine in an application instead of one badge component per
  enum.

Requires React 18.3+ or 19, Tailwind CSS v4, and daisyUI v5.

```sh
pnpm add @vaam-apps/ui
```

## Setup

Two steps, and skipping the second is the most common way to end up with
an unstyled page.

**1. Import the token layer** into your global stylesheet, *after*
Tailwind and the daisyUI plugin:

```css
@import "tailwindcss";
@plugin "daisyui";
@import "@vaam-apps/ui/styles/theme.css";
@source "../node_modules/@vaam-apps/ui/dist";
```

**2. Keep the `@source` line.** Tailwind v4 generates only the utilities
it can see used, and it does not look inside `node_modules` on its own.
Without it every component renders with no styling at all — no error, no
warning, just a page that looks broken in a way that is miserable to
debug. Adjust the relative path to reach your own `node_modules`.

The theme registers itself under daisyUI's `dark` name and sets
`prefersdark`, so `<html data-theme="dark">` (or no attribute at all)
picks it up.

## What is in it

### Primitives

`Button` · `Input` · `Textarea` · `Label` · `FormField` · `Select` ·
`RadioGroup` · `ChipSelect` · `Checkbox` · `Switch` · `Calendar` ·
`DatePicker` · `DateRangePicker` · `Dialog` · `ConfirmDialog` · `Drawer` ·
`Popover` · `DropdownMenu` · `CommandMenu` · `Tooltip` · `Toast` ·
`Table` · `Tabs` · `Pagination` · `Card` · `Badge` · `Separator` ·
`Skeleton` · `Spinner` · `Progress` · `SideNav` · `InlineConfirm`

### Data display

`Money` · `IdDisplay` · `PhoneDisplay` · `MaskedValue` · `CopyButton` ·
`TimestampDisplay` · `DetailRow` / `DetailList` · `StatTile` · `Code`

### Patterns

`ScreenStack` / `ScreenHeader` · `InlineBanner` · `InlineEmptyState` ·
`StaleWriteBanner` · `RouteSkeleton` · `PayloadInspector` ·
`StateTimeline` · `LiveRow`

### Status

`StatusPill` · `createStatusPill` · `StateMark` · `StateChip` ·
`defineStatusSystem` · `HUE_CLASSES`

## The status system

A status system is one state machine's presentation: every state it can
be in, mapped to a glyph, a hue, an attention level and human copy. The
library owns the vocabulary and the rendering. **The table belongs to
you**, because what a state means is domain knowledge.

```tsx
import { createStatusPill, defineStatusSystem } from "@vaam-apps/ui";

export const ORDER_STATUS = defineStatusSystem({
  pending: {
    family: "in-flight", silhouette: "circle", mark: "pie-1",
    hue: "neutral", filled: false, attention: "quiet",
    label: "Pending", tooltip: "Awaiting payment.",
  },
  paid: {
    family: "terminal", silhouette: "circle", mark: "check",
    hue: "success", filled: true, attention: "quiet",
    label: "Paid", tooltip: "Settled and captured.",
  },
  disputed: {
    family: "unresolved", silhouette: "diamond", mark: "question",
    hue: "uncertain", filled: false, attention: "loud",
    label: "Disputed", tooltip: "The payer opened a chargeback.",
  },
});

export const OrderStatusPill = createStatusPill(ORDER_STATUS);
```

```tsx
<OrderStatusPill state="disputed" showLiteral />
```

`state` accepts exactly that machine's literals, so a status from a
different machine is a compile error. Add a second machine by calling
`createStatusPill` again — the rendering is shared, the vocabularies stay
apart.

Three things about it are deliberate:

- **Each state differs in silhouette, interior mark and fill as well as
  hue.** Colour alone fails for the ~8% of men with a colour-vision
  deficiency and fails completely in a monochrome screenshot pasted into
  a ticket.
- **`family` is presentational, never authorisation.** A UI that greys
  out "cancel" because its own table says `terminal` will be wrong the
  first time the state machine changes and nobody remembers the table
  exists. Propose the action; let the server refuse it.
- **`unresolved` is a first-class outcome**, distinct from success and
  failure. Systems that model only two outcomes end up reporting "we
  never learned what happened" as whichever is more convenient.

## Money

```tsx
import { formatMoney, Money } from "@vaam-apps/ui";

<Money amount={500_000} currency="XAF" />        // XAF 500,000
<Money amount="1250" currency="USD" />           // USD 12.50
formatMoney(1250, "KWD");                        // KWD 1.250
```

Amounts go in as an **integer count of minor units** — `number`,
`bigint`, or a digit string, so an `i64` from a backend survives the
trip. The decimal exponent comes from `Intl`, which knows every ISO 4217
currency including the three-decimal ones (`KWD`, `BHD`, `TND`) that
hand-written tables invariably forget. Scaling is string surgery, never
division, so an amount past `Number.MAX_SAFE_INTEGER` formats exactly.
A non-integer throws rather than being silently rounded.

## Dates

`DatePicker` and `DateRangePicker` wrap `react-day-picker` in a popover
and exchange **`YYYY-MM-DD` strings, not `Date` objects**. A `Date` is an
instant, and an instant rendered in another zone is a different calendar
day — which is how a filter for "today" quietly returns yesterday's rows
for anyone west of the server. `Date` is confined to the internals.

```tsx
const [range, setRange] = useState<IsoDateRange | undefined>();
<DateRangePicker value={range} onValueChange={setRange} />
```

`Calendar` is the bare, themed `DayPicker` if you need a different shell.

## `cn()`

`clsx` + `tailwind-merge`, extended so the later class wins for this
theme's own scales and for daisyUI's component modifiers:

```ts
cn("btn btn-primary", "btn-ghost");      // "btn btn-ghost"
cn("btn btn-primary", "btn-outline");    // both — orthogonal in daisyUI 5
cn("rounded-sm", "rounded-field");       // "rounded-field"
cn("text-caption", "text-danger-fg");    // both — size and colour
```

That last case is the one worth knowing about: stock `tailwind-merge`
classifies an unrecognised `text-*` value as a colour, so a custom font
size sitting beside a colour is silently deleted. The scale is registered
here so it is not.

## Design constraints

Worth knowing before you fight them:

- **Borders, not shadows.** Cards and panels are a 1px border on a
  surface step. Shadows are reserved for genuinely floating layers
  (popover, dialog).
- **There is no success or warning *button*.** Those hues belong to
  status; a button is primary, secondary, ghost or destructive. Mixing
  them is how a status language erodes.
- **Empty states are inline status lines, not centred placards** — see
  `InlineEmptyState`.
- **Dark only.** There is one theme, and no toggle. A second theme is a
  real amount of work to keep honest and nothing here pretends to have
  done it.

## Versioning

Pre-1.0. Minor versions may contain breaking changes; pin exactly if that
matters to you. See `CHANGELOG.md`.

## Licence

MIT.
