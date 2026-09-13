# Layout, navigation and theming

The parts that decide where things sit on a screen: the surfaces a record
is read on, the table it is scanned in, the navigation around it, and the
theme it is painted in. Everything imports from `@vaam-apps/ui`.

Two of these components have a contract the caller has to hold up, and
both fail silently when it is not held: `SideNav` does not reserve its own
space, and `Table`'s sticky header does nothing without `maxHeight`. Those
two sections are the ones to read before writing a screen.

## `Card` / `CardHeader` / `CardBody`

The diagnostic surface: a hairline border on a `--surface-3` step, no
shadow. This is the default container for a section of a screen — a
provider's configuration, a delivery's detail, a group of related fields.

| Prop | Type | Notes |
|---|---|---|
| `Card` `glow` | `boolean` | Aurora glow. The instrument register only. Default `false`. |
| `CardHeader` `title` | `React.ReactNode` | Rendered as a heading, truncated to one line. |
| `CardHeader` `headingLevel` | `2 \| 3 \| 4 \| 5 \| 6` | Default `3`. |
| `CardHeader` `meta` | `React.ReactNode` | A mono line under the title — ids, priorities, counts. |
| `CardHeader` `action` | `React.ReactNode` | Right-aligned slot, usually buttons. |

`headingLevel` is a prop rather than a constant because a heading level
belongs to the page's outline, not to the card. A card placed directly
under the screen's `<h1>` needs `headingLevel={2}`; leaving the default
`h3` there skips a level, which axe reports as `heading-order` and a
screen-reader user experiences as a missing section.

**`glow` carries no meaning.** It cannot be tinted per state and nothing
should be read into its presence — the hue vocabulary stays entirely with
the status system. It is for a surface whose job is to make one number
findable, and a screen where every card glows has no glow.

```tsx
import { Card, CardHeader, CardBody, DetailList, DetailRow } from "@vaam-apps/ui";

<Card>
  <CardHeader
    title="Orange CM — outbound SMS"
    meta="priority 100 · weight 3 · msisdn 237*"
    headingLevel={2}
    action={<Button variant="secondary" size="sm">Disable route</Button>}
  />
  <CardBody>
    <DetailList>
      <DetailRow label="Endpoint" value="api.orange.com/smsmessaging/v1" />
      <DetailRow label="Last success" value={<TimestampDisplay value={lastSuccess} />} />
    </DetailList>
  </CardBody>
</Card>
```

## `Table` / `TableHeader` / `TableBody` / `TableRow` / `TableHead` / `TableCell`

The list a person scans for the one row that is wrong. No zebra striping —
the status tints are the signal, and stripes plus tints are noise. Row
dividers are a hairline; hover is a surface step.

| Prop | Type | Notes |
|---|---|---|
| `Table` `maxHeight` | `string \| undefined` | e.g. `"24rem"`. **Required for a sticky header.** |
| `Table` `label` | `string \| undefined` | Names the scroll wrapper, used only while it is really scrollable. |
| `TableRow` `selected` | `boolean` | Surface step plus a 2px inset marker on the leading edge. |
| `TableHead` / `TableCell` `align` | `"start" \| "end"` | `"end"` for money and counts. |
| `TableCell` `mono` | `boolean` | Mono, `tabular-nums` — ids, amounts, anything compared column-wise. |
| `TableHead` / `TableCell` `hideBelow` | `Breakpoint` (`"sm" \| "md" \| "lg" \| "xl"`) | Drops the column below that width. Put it on the `TableHead` **and** every `TableCell` in that column. |

### The sticky header needs `maxHeight`

`TableHeader` is `sticky top-0`, and `sticky` needs a scrollport. The
wrapper `Table` renders is already the nearest scroll container, but at
its default `height: auto` it never overflows, so it never scrolls — the
page does, and the header travels with it. Measured live by reading
`thead.getBoundingClientRect().top` before and after a 300px page scroll:
`72` → `-228`, a 1:1 delta, i.e. no stickiness at all. With
`maxHeight="20rem"` and the wrapper itself scrolled by 300px, the same
read was `16` before and `16` after.

Omitting `maxHeight` is a legitimate choice for a short table. It is only
a bug when you also expected the header to stay.

### `label`, and why it is usually absent

The component measures `scrollWidth`/`scrollHeight` against
`clientWidth`/`clientHeight` on mount and on resize, and only a wrapper
that is actually overflowing right now becomes keyboard-focusable. Pass
`label` when the table is genuinely expected to scroll — a bounded one, or
a wide one on a narrow screen. With `label`, that wrapper becomes a named
`region` landmark; without it, a scrollable wrapper is still focusable but
stays roleless, because an unnamed landmark is worse than no landmark.

```tsx
import {
  Table, TableHeader, TableBody, TableRow, TableHead, TableCell,
} from "@vaam-apps/ui";

<Table maxHeight="24rem" label="Delivery attempts">
  <TableHeader>
    <TableRow>
      <TableHead>Status</TableHead>
      <TableHead>Message</TableHead>
      <TableHead hideBelow="md">Provider</TableHead>
      <TableHead align="end">Cost</TableHead>
    </TableRow>
  </TableHeader>
  <TableBody>
    {attempts.map((attempt) => (
      <TableRow key={attempt.id} selected={attempt.id === selectedId}>
        <TableCell>
          <DeliveryPill state={attempt.state} />
        </TableCell>
        <TableCell mono>{attempt.messageId}</TableCell>
        <TableCell hideBelow="md">{attempt.provider}</TableCell>
        <TableCell align="end">
          <Money amount={attempt.cost} currency="XAF" />
        </TableCell>
      </TableRow>
    ))}
  </TableBody>
</Table>
```

## `ValueTabs` / `ValueTabsList` / `ValueTabsTrigger` / `ValueTabsContent`

Underline tabs, addressed **by value rather than by index** — the panel a
tab shows does not change when someone reorders the list. There is no pill
or segmented variant; segmented controls are consumer furniture.

| Prop | Type | Notes |
|---|---|---|
| `ValueTabs` `value` | `string \| undefined` | Controlled. |
| `ValueTabs` `defaultValue` | `string \| undefined` | Uncontrolled; falls back to the first trigger. |
| `ValueTabs` `onValueChange` | `((value: string) => void) \| undefined` | |
| `ValueTabsList` `className` | `string` | Lands on the tablist row — `gap-*`, `justify-*`, `border-b-*` belong here. |
| `ValueTabsList` `wrapperClassName` | `string` | Lands on the horizontal scroller around the row. |
| `ValueTabsTrigger` `value` | `string` | Read from the element tree, not inside the trigger. |
| `ValueTabsContent` `value` | `string` | Kept for parity. **The real pairing is positional** — panels must be declared in trigger order. |

Note the explicit `| undefined` on the optional props. This package
compiles under `exactOptionalPropertyTypes`, where `value?: string` means
"you may omit the key", not "you may pass `undefined`" — so the ordinary
`const [tab, setTab] = useState<string>()` then `value={tab}` pattern
would be a type error without it.

```tsx
import {
  ValueTabs, ValueTabsList, ValueTabsTrigger, ValueTabsContent,
} from "@vaam-apps/ui";

<ValueTabs defaultValue="attempts">
  <ValueTabsList>
    <ValueTabsTrigger value="attempts">Attempts</ValueTabsTrigger>
    <ValueTabsTrigger value="payloads">Payloads</ValueTabsTrigger>
    <ValueTabsTrigger value="audit">Audit</ValueTabsTrigger>
  </ValueTabsList>
  <ValueTabsContent value="attempts">{attemptsTable}</ValueTabsContent>
  <ValueTabsContent value="payloads">{payloadInspector}</ValueTabsContent>
  <ValueTabsContent value="audit">{auditTrail}</ValueTabsContent>
</ValueTabs>
```

## `Pagination`

Previous/next paging with a position readout that does not lie.

| Prop | Type | Notes |
|---|---|---|
| `onPrevious` / `onNext` | `(() => void) \| undefined` | Required keys, nullable values: `undefined` disables that direction. |
| `position` | `PaginationPosition` | `{ kind: "offset", offset, pageSize, total }` or `{ kind: "cursor", count }`. |
| `children` | `React.ReactNode` | Between the readout and the buttons — a page-size select, a "Newest first" note. |
| `label` | `string \| undefined` | Default `"Pagination"`. |

The two `position` shapes are the point. `offset` renders `1–25 of 340`
and is only honest when the API really returns a total; `cursor` renders
`25 shown` and no page numbers, because a keyset-paged endpoint — which
most event-shaped endpoints are, since counting a large table per request
is the expensive part — has no honest page number to render. Inventing one
produces a "Page 3 of ?" that is wrong the moment a row is inserted.

The callbacks are required-but-nullable so a caller has to decide what the
boundary is, rather than getting a silently dead button by omission.

Pass `label` whenever a screen shows more than one pager — controls above
and below a table is the usual case. Two landmarks both called
"Pagination" is a real navigation defect: a screen-reader user listing
landmarks cannot tell which is which.

```tsx
import { Pagination } from "@vaam-apps/ui";

<Pagination
  position={{ kind: "cursor", count: attempts.length }}
  onPrevious={cursors.previous ? () => goTo(cursors.previous) : undefined}
  onNext={cursors.next ? () => goTo(cursors.next) : undefined}
  label="Delivery attempts, bottom"
/>
```

## `Separator`

A rule between two groups of content. `orientation` is `"horizontal"`
(default) or `"vertical"`; `decorative` defaults to `true`.

`decorative` is the whole API. `true` renders a `<div role="none">` — a
purely visual rule that assistive tech ignores, which is right when the
groups either side are already distinct (a card, a heading). `false`
renders a real `<hr>` with `aria-orientation`, for the case where the rule
is the *only* thing saying the content changed. It is an `<hr>` rather
than `role="separator"` on a `<div>` because the ARIA role is the
interactive, resizable-divider variant and wants `tabIndex` and
`aria-valuenow`; `<hr>` carries the static semantics natively.

## `SideNav` — three shapes, and the padding it cannot set for you

The console's primary navigation, built from data. It owns no routes: the
caller passes items and the current path, resolved by its own router.

```ts
interface NavItem { label: string; href: string; icon: ComponentType<{ size?: number }> }
interface NavGroup { label: string; items: NavItem[] }
```

| Prop | Type | Notes |
|---|---|---|
| `topItem` | `NavItem` | Flat, ungrouped, always first — the dashboard row. |
| `groups` | `NavGroup[]` | Section header plus rows. Headers show only in the sidebar. |
| `footerItems` | `NavItem[]` | De-emphasised utility rows. Administrivia, not content. |
| `currentPath` | `string` | A plain string — this package has no router dependency. Active means equal, or a path prefix followed by `/`. |
| `accountSlot` | `React.ReactNode` | Rendered as-is; never built here. Shown only where there is room for it (the sidebar, and the off-canvas tree). |
| `smallScreen` | `"floating" \| "off-canvas" \| undefined` | Default `"floating"`. |
| `collapsed` | `boolean \| undefined` | Turns the sidebar back into the rail. |

### The three shapes

- **Below 640px** — a horizontal pill along the bottom: four destinations
  and an overflow menu for the rest. Four is a thumb measurement, not a
  taste one: five 44px targets, four 4px gaps and 12px of pill padding
  come to 248px, which still reads as a pill on a 375px phone. A 52px
  column down the side of that phone would be 14% of the width,
  permanently, in the thumb's dead zone — so the rail turns rather than
  shrinks. The overflow rows are real anchors, so middle-click and
  ⌘-click still work on them.
- **640–1279px** — a vertical floating rail, 52px wide, inset 12px from
  the left edge, vertically centred. Labels come from a native `title` on
  hover, and from `sr-only` text for a screen reader. (A CSS tooltip
  cannot work here: the rail is a scroll container and clips its own
  bubble — measured at 1100px, the label painted from 32px to 115px
  inside a 40px-wide clip box, so no label ever appeared.)
- **1280px and up** — the full sidebar, with labels and group headers, in
  flow. With `collapsed`, this band uses the floating rail instead.

### Only the sidebar takes space out of the page

Both rails are `position: fixed` and portalled to `document.body`. That is
deliberate — a `fixed` element's containing block is any ancestor with a
`transform`, `filter`, `backdrop-filter`, `contain` or
`will-change: transform`, and this package's own `Drawer` is exactly that
(vaul stamps `will-change: transform` on every drawer unconditionally), so
a rail mounted inside one used to re-anchor to the drawer instead of the
viewport. Portalling makes that impossible.

The consequence is the thing integrators get wrong: **a portalled `fixed`
rail cannot reserve its own space, so the content column's padding is
yours to set.** Nothing errors when you forget. The screen just has a rail
sitting on top of its first column of text, or a last table row hidden
under the bottom pill.

What to reserve, from the rail's own geometry: the vertical rail occupies
the leftmost 64px (12px inset plus 52px wide), so a left pad of 80px
(`pl-20`) leaves a 16px gutter. The bottom pill is 44px targets plus 6px
of padding, 12px off the bottom — about 68px, so `pb-24` clears it. Below
640px there is no rail on the left at all, and above 1279px (uncollapsed)
the sidebar is in flow and needs no gutter:

```tsx
import { SideNav, ScreenStack, ScreenHeader } from "@vaam-apps/ui";

<div className="flex min-h-dvh">
  <SideNav
    topItem={{ label: "Dashboard", href: "/", icon: LayoutDashboard }}
    groups={[
      { label: "Messaging", items: [
        { label: "Messages", href: "/messages", icon: MessageSquare },
        { label: "Routes", href: "/routes", icon: Waypoints },
      ] },
      { label: "Providers", items: [
        { label: "Providers", href: "/providers", icon: Server },
      ] },
    ]}
    footerItems={[{ label: "Settings", href: "/settings", icon: Settings }]}
    currentPath={pathname}
    accountSlot={<AccountRow />}
  />

  {/* The rails float over this column. These paddings are the caller's job. */}
  <main className="min-w-0 flex-1 overflow-y-auto p-6 pb-24 sm:pb-6 sm:pl-20 xl:pl-6">
    <ScreenStack>{children}</ScreenStack>
  </main>
</div>
```

If you pass `collapsed`, the sidebar never renders and the rail is the
navigation at every width from 640px up — so keep the left gutter at `xl`
too (`sm:pl-20` with no `xl:pl-6` override) for as long as the preference
is on. `collapsed` is a JS boolean rather than a breakpoint because it is
a preference the caller owns, persists, and usually puts a toggle beside.

`smallScreen="off-canvas"` is the opt-out: it mounts a full-label,
in-flow accordion tree below 1024px for a caller who already owns a drawer
and wants this component to fill it rather than float over it. In that
mode every band is in flow, `collapsed` is ignored, and no rail is
mounted at all.

One SSR note: everything except the floating rails is in the
server-rendered HTML — the sidebar, and the whole off-canvas tree. The
rails cannot be, because a portal needs a `document`, so they mount
after hydration.

## `ScreenStack` / `ScreenHeader`

The page scaffold, so a screen does not rebuild it. `ScreenStack` is the
vertical rhythm every screen composes its sections into: a `gap-6` flex
column with `min-w-0`. That `min-w-0` is load-bearing — a flex item
defaults to `min-width: auto`, so one wide child (a table with a long id
column, a `<pre>` of an unwrapped payload) would push the whole screen
wider than the viewport instead of scrolling inside its own box. That is
the "the page scrolls sideways and nobody can see why" failure, fixed once
here instead of per section.

`ScreenHeader` takes `title` and an optional `description` and renders the
screen's one `<h1>` plus a clamped two-line description. The clamp is the
decision: the description is documented as one line, and a header that
silently grows to five pushes content off the fold on exactly the screens
whose author wrote the longest prose. The full text stays in the DOM.

```tsx
<ScreenStack>
  <ScreenHeader
    title="Routes"
    description="Which provider carries which traffic, and why that one won."
  />
  <Card>
    <CardHeader title="Catch-all" meta="priority 500 · weight 1" headingLevel={2} />
    <CardBody>{routeDetail}</CardBody>
  </Card>
</ScreenStack>
```

The `headingLevel={2}` above is not decoration: `ScreenHeader` has already
spent the `h1`.

## `Badge`

A non-status tag: app name, provider key, role, environment, sender id.
Mono, caption-sized. `variant` is `"neutral"` (default) or `"outline"`.

**Never use `Badge` for a state.** That is `StatusPill`'s job, and mixing
the two is how the status language erodes — a reader who has learned that
coloured chips mean states starts having to read every one of them.

```tsx
<Badge>orange-cm</Badge>
<Badge variant="outline">staging</Badge>
```

## `Progress` and `Spinner`

Two different honesty claims about a wait.

`Progress` is a determinate bar for a **bounded, countable** quantity —
attempts against a retry budget, rows processed, quota consumed.

| Prop | Type | Notes |
|---|---|---|
| `value` | `number` | Clamped into `[0, max]`. |
| `max` | `number` | Default `100`, so a percentage needs no ceremony. |
| `label` | `string` | Required. The accessible name, e.g. `"Retry budget used"`. |
| `tone` | `StatusHue` | Default `"neutral"`. Leave it unless the number's colour carries meaning. |
| `showValue` | `boolean` | Renders `3 / 5` beside the bar. |

A bar creeping toward a value it cannot reach is a lie, which is why
`Spinner` exists for the unbounded case: a button mid-submit, a poll with
no placeholder to draw. Its `size` is `"xs" | "sm" | "md" | "lg"` (default
`"sm"`), and `label` is optional — omitted, the spinner renders
`aria-hidden`, which is correct when a visible sibling already says what
is loading. Two announcements of one wait is worse than one.

```tsx
<Progress value={attempt} max={5} label="Delivery attempts used" showValue tone="warning" />
<Spinner size="xs" />
```

Where the shape of what is arriving is known, neither of these is the
answer — use `Skeleton`.

## `Skeleton` / `SkeletonText`

A placeholder for content whose shape is known but whose value has not
arrived. Match the real element's height exactly, so nothing shifts when
it lands.

| Prop | Type | Notes |
|---|---|---|
| `animated` | `boolean` | Default `true`. `false` is the flat block, for a placeholder inside something already moving. |
| `SkeletonText` `lines` | `number` | Default `3`. |
| `SkeletonText` `lineClassName` | `string` | Match the real text's line-height. |

**It animates itself — do not add a pulse.** What runs is not a shimmer:
a shimmer is a highlight sweeping on a fixed period, which reads as a
progress indicator and makes twenty rows pulse in unison like a metronome.
This is two oversized, very low-contrast gradient fields drifting past
each other on coprime periods, with no edge, no direction and no shared
phase. It says "still waiting, not stuck" and deliberately says nothing
about how much longer. It goes still under `prefers-reduced-motion`, from
the stylesheet — no call site has to decide that.

`SkeletonText`'s last line is short on purpose, which is why it is a
component rather than a loop: a stack of equal-width bars reads as a table
or a list, while real prose ends mid-line. The widths are a fixed cycle
rather than random, because a random width differs between server and
client and that is a hydration mismatch.

Skeletons are `aria-hidden` — a picture of absent content, and reading out
a dozen empty boxes is worse than silence. The *wait* still has to be
announced: `RouteSkeleton` carries the `role="status"` live region for the
screen-level case, and any other caller standing a skeleton in for real
content should do the same one level up.

```tsx
<Card>
  <CardHeader title="Provider" />
  <CardBody>
    <Skeleton className="h-8 w-48" />
    <SkeletonText lines={4} className="mt-4" />
  </CardBody>
</Card>
```

## Theming: `useTheme`, `ThemeSwitcher`, `setThemePreference`, `THEME_STORAGE_KEY`, `themeInitScript`

Five exports, one store. The preference is `"system" | "light" | "dark"`
and it is three states rather than two on purpose: `dark` carries
`prefersdark`, so the package already has an opinion about following the
OS. A plain two-way toggle has to start somewhere, and wherever it starts
is a decision the operator never made — "system" stops being expressible
the moment a consumer wires one up.

- **`ThemeSwitcher`** — the packaged control, a three-option radio group.
  Props: `aria-label` (default `"Theme"`), `className`, and `persist`
  (default `true`).
- **`useTheme(options?)`** — the headless half, for a settings row, a
  command-menu action or a keyboard shortcut. Returns `preference` (what
  the operator asked for — use it to decide which control is checked),
  `resolvedTheme` (`"light" | "dark"`, with `"system"` resolved against
  the OS query — use it to render "currently dark" copy), and
  `setPreference`.
- **`setThemePreference(next, options?)`** — the same setter outside
  React. `options.persist` (default `true`) gates only the storage write;
  the document re-themes either way. Pass `false` where a preference must
  not outlive the caller.
- **`THEME_STORAGE_KEY`** — `"vaam-ui:theme"`. Exported so a consumer's own
  settings UI can read or clear the same key without retyping it. Every
  read and write in this module is wrapped in `try`/`catch`: Safari
  private windows throw on `setItem`, and some webviews and org policies
  block storage entirely. Losing persistence for a session is acceptable;
  throwing out of a click handler is not.

Changes made in another tab arrive here: the store listens for `storage`
(which fires on every *other* tab sharing the origin, never the one that
made the write) and for the OS colour-scheme query.

### `themeInitScript` — the flash, and how to render it

React cannot run before the browser's first paint, so no component can
prevent one frame of the wrong theme. The server emits no `data-theme`, so
the browser paints the default (`dark`) first, and a visitor who chose
light sees a dark flash on every full page load. The fix has to be a
synchronous, blocking script in `<head>`, before any stylesheet that
paints a surface colour:

```tsx
// app/layout.tsx — the script goes first in <head>.
import { themeInitScript } from "@vaam-apps/ui";

<html lang="en" suppressHydrationWarning>
  <head>
    <script
      // biome-ignore lint/security/noDangerouslySetInnerHtml: a fixed literal
      dangerouslySetInnerHTML={{ __html: themeInitScript }}
    />
  </head>
  <body>{children}</body>
</html>
```

`suppressHydrationWarning` on `<html>` is required, not a defensive habit:
the script mutates `data-theme` on that exact element before React runs,
so the attribute React observes during hydration never matches the
server-rendered markup. It scopes to that one element's own attributes and
text — nothing nested inside it loses hydration checking.

**Inline the constant verbatim.** It is one literal string with nothing
interpolated into it, and it must stay that way: it is handed to
`dangerouslySetInnerHTML` and executed, so every interpolation into it is
a script-injection sink, and a code scanner flags it as code construction
from a non-literal value. It used to build itself from the storage key,
the attribute name and the media query; those are pinned by a test that
asserts the literal contains each constant's exact JSON form, so a rename
fails the build rather than silently shipping a script that reads the
wrong key.

The script never throws — on blocked storage it leaves the attribute unset
and falls through to the default theme.
