# Overlays

Everything that appears over a screen: dialogs, drawers, popovers, menus,
the command palette, tooltips and toasts. All import from
`@vaam-apps/ui`.

The hard part is not the props. It is that six of these can carry the
same content, and choosing the wrong one costs the operator something
specific — a lost place in a list, a confirmation they stopped reading
months ago, a message that expired while they were looking at a payload.

## Choosing one

| The operator is…                                                                | Use                 | Not                                             |
| ------------------------------------------------------------------------------- | ------------------- | ----------------------------------------------- |
| deciding something irreversible, and must stop                                  | `ConfirmDialog`     | a toast                                         |
| confirming a row-level action                                                   | `InlineConfirm`     | `ConfirmDialog`                                 |
| confirming something **inside an open drawer**                                  | `InlineConfirm`     | a nested `Dialog` — it does not work, see below |
| reading one record's headline without leaving the list                          | `QuickDetailDrawer` | `Dialog`                                        |
| working through a whole record — every field, an edit form, destructive actions | `MoreDetailDrawer`  | `Dialog`                                        |
| taking one focused action with a form in it                                     | `Dialog`            | a drawer                                        |
| picking from a short list of actions on a row                                   | `DropdownMenu`      | `Popover`                                       |
| reading a little extra anchored content, possibly interactive                   | `Popover`           | `Tooltip`                                       |
| reading a one-line gloss on a label                                             | `Tooltip`           | `Popover`                                       |
| jumping somewhere by typing                                                     | `CommandMenu`       | `DropdownMenu`                                  |
| being told something already finished                                           | `toast`             | anything modal                                  |

Two rules that cut most of the wrong answers:

- **A modal is an interruption, and an interruption that appears every
  time is one people learn to dismiss unread.** `ConfirmDialog`'s own
  source says so outright: reserve it for actions that are expensive and
  hard to reverse, and let `InlineConfirm` handle the rest.
- **Anything the operator must act on is never a toast.** A toast expires
  on a timer (4s by default, capped at four on screen); a message that
  expires while someone is reading a payload is a message that was never
  delivered. Toasts are for "copied", "requeued", "saved".

---

## `Dialog` — a modal for one focused decision

Parts: `Dialog`, `DialogTrigger`, `DialogContent`, `DialogFullScreen`,
`DialogHeader`, `DialogTitle`, `DialogDescription`, `DialogActions`,
`DialogClose`.

It is a **compound component**, like `Select`: `Dialog` owns the open
state, and everything visible is a part you nest inside it. You pick the
*presentation* by the container part; the other parts are the same in
both.

`Dialog` itself is a context provider, not a rendered element — it holds
the open state so that both shapes work: uncontrolled, driven by a
`DialogTrigger` rendered in place, or fully controlled from the screen's
own state.

```ts
interface DialogProps {
  open?: boolean | undefined;          // controlled
  defaultOpen?: boolean | undefined;   // uncontrolled
  onOpenChange?: ((open: boolean) => void) | undefined;
  children: ReactNode;
}
```

The `| undefined` on each is deliberate: this package compiles under
`exactOptionalPropertyTypes`, where a bare `open?: boolean` would reject
`open={maybeUndefined}`. Yours can pass it.

### The container — pick one

- **`DialogContent`** — M3's basic dialog at every width: centred, 28px
  corners, 560px wide where there is room (the screen less 16px each side
  on a phone), on `surface-3`. Use it for anything short: a confirmation,
  a few fields.
- **`DialogFullScreen`** — M3's full-screen dialog **below 640px**, the
  same basic dialog from 640px up. Use it for a long or form-heavy dialog
  a phone should give the whole screen. Below 640px: a 64px top bar leads
  with a close icon and carries your `DialogActions` trailing, and a
  `DialogClose` among the actions is hidden (the close icon is that
  action). Chosen by CSS media query, so it server-renders correctly.

`className` on either lands on the panel, and it is the only prop they
take besides `children` — the parts carry nothing DOM-only, so a native
implementation can share them. The panel is bounded at `max-h-[85vh]`
(full height when full-screen) and scrolls internally.

**Inside a dialog, surfaces step up one.** The panel is `surface-3`, which
is also this library's hover and selected fill — so everything inside it
reads `surface-1` as `surface-2`, `surface-2` as `surface-3`, and
`surface-3` as a `surface-4` half a step on (lighter in the dark theme,
darker in the light one — as far as text contrast allows). A `RadioGroup`'s checked
row, a table's hover, a switch track keep their meaning without you doing
anything. Do not compensate with your own fills. (Full-screen on a phone
the panel is the page's own ground, and nothing shifts.)

### The parts

- **`DialogHeader`** — holds `DialogTitle` and `DialogDescription`, pinned
  to the top of the visible panel while a long body scrolls.
- **`DialogTitle`** — the headline and the dialog's accessible name.
  Always include it.
- **`DialogDescription`** — the supporting text, what `aria-describedby`
  points at.
- **`DialogActions`** — the dialog's buttons, end-aligned, pinned to the
  bottom of the visible panel (in the full-screen bar below 640px). Put the
  dismissing one in as a `DialogClose`, so the full-screen dialog knows to
  hide it. Write it as a direct part of the container — inside your own
  `relative` (or otherwise positioned) wrapper it never reaches the bar.
  The bar holds **one short confirming action**, as M3's does. More fit —
  three short labels at 320px — but they crowd the bar, and a long label
  leaves little room for another. **Renamed from
  `DialogFooter` in 0.3.0** — same place, same children.
- **`DialogClose`** — closes the dialog, two ways:
  - **Bare** (`<DialogClose />`, no children, no `as`): the close icon.
    Every dialog renders one already — the ✕ in the top-right corner, or
    the bar's leading close icon when full-screen. Written as a **direct
    part** of `DialogContent`/`DialogFullScreen` (or inside a fragment
    there), it *replaces* that icon — the way to change its `aria-label`
    (default "Close"). Written anywhere else — inside `DialogActions`, the
    header, your own element — it is an extra ✕ icon button where you put
    it, and the corner icon stays.
  - **With children or `as`**: closes on click and renders what you give
    it, e.g. `<DialogClose as={Button} variant="ghost">Cancel</DialogClose>`.

`DialogTrigger` and `DialogClose` are polymorphic — `as={Button}` rather
than Radix's old `asChild`. Both set `type="button"` unless `as` is an
intrinsic tag that is not a button (`"a"`, `"div"`), so neither submits a
surrounding `<form>`; pass `type="submit"` yourself if you want that.

```tsx
<Dialog>
  <DialogTrigger as={Button} variant="secondary" size="sm">
    Requeue payout
  </DialogTrigger>

  <DialogContent>
    <DialogHeader>
      <DialogTitle>Requeue payout pay_01J8ZQ4M?</DialogTitle>
      <DialogDescription>
        The provider returned insufficient_float at 14:02 UTC. Requeuing submits the
        same amount to the same account — it does not create a second payout.
      </DialogDescription>
    </DialogHeader>

    <DetailList>
      <DetailRow label="Amount" value={<Money amountMinor={4500000} currency="XAF" />} />
      <DetailRow label="Provider" value="orange_cm" />
    </DetailList>

    <DialogActions>
      <DialogClose as={Button} variant="ghost" size="sm">
        Cancel
      </DialogClose>
      <Button type="button" size="sm" onClick={requeue}>
        Requeue
      </Button>
    </DialogActions>
  </DialogContent>
</Dialog>
```

A form a phone should give the whole screen — the same parts inside
`DialogFullScreen`, the submit button pointing at the form with `form=`:

```tsx
<Dialog open={open} onOpenChange={setOpen}>
  <DialogFullScreen>
    <DialogHeader>
      <DialogTitle>New webhook endpoint</DialogTitle>
      <DialogDescription>The signing secret is shown once, after creation.</DialogDescription>
    </DialogHeader>
    <form id="create-endpoint" onSubmit={create}>{/* FormFields */}</form>
    <DialogActions>
      <DialogClose as={Button} variant="ghost" size="sm">Cancel</DialogClose>
      <Button type="submit" form="create-endpoint" size="sm">Create</Button>
    </DialogActions>
  </DialogFullScreen>
</Dialog>
```

**Migrating from 0.2.x:**

- Rename `DialogFooter` to `DialogActions`.
- Write a Cancel button as `<DialogClose as={Button} variant="ghost">`
  rather than a `Button` whose `onClick` closes the dialog — that is how the
  full-screen dialog knows to hide it. If your Cancel also ran logic, keep
  it as `onClick` on the `DialogClose`: it runs, then the dialog closes; do
  not *also* call your close handler there, or it runs twice. A button that
  must stay visible in the full-screen bar ("I've saved it — close") stays
  a plain `Button`.
- The panel is now M3's: 28px corners, `surface-3`, 560px wide by default
  (was 480px; `ConfirmDialog` was 384px), a 1px ring instead of a border,
  and it fades in without scaling. A `max-w-*` you pass in `className`
  still wins over the default: drop `max-w-[560px]` (now the default), and
  decide for any other width (`max-w-[480px]`, `max-w-sm`) whether you
  still want it.
- Type and spacing change: `DialogTitle` is 20px (was 16px), 16px above
  the description (was 4px); with a mouse the padding is 20px (was 24px)
  and the buttons sit 16px under the text.
- The parts take `className` and `children` only; any other attribute you
  passed to `DialogContent`, `DialogHeader` or `DialogFooter` is dropped.
- Consider `DialogFullScreen` for your long, form-heavy dialogs.

**You do not have to solve tall content.** `DialogHeader` and
`DialogActions` are `sticky`, pinned to the visible panel while the body
scrolls between them, and the close icon is a sibling of the scrollport
rather than inside it, so it never travels out of view. Do not add your
own `overflow-y-auto`; putting it on the panel is the exact bug this
layout was built to avoid.

A `Select` inside a dialog works: its dropdown floats over the panel
rather than being clipped by it — on the panel's own material, told apart
by its border and shadow, the way a sheet opened over a sheet is the same
material as the one under it — and on a phone its sheet opens over the
dialog.

---

## `ConfirmDialog` — the yes/no worth interrupting someone over

A composition over `Dialog`, so nothing above needs repeating.

```ts
interface ConfirmDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: ReactNode;
  description: ReactNode;             // say "this cannot be undone" if it cannot
  confirmLabel: string;               // names the action; never "OK"
  cancelLabel?: string;               // default "Cancel"
  tone?: "default" | "destructive";
  onConfirm: () => void;
  busy?: boolean;                     // both buttons disabled, confirm shows a Spinner
  children?: ReactNode;               // rendered above the buttons — a failed attempt's message
}
```

```tsx
const [open, setOpen] = useState(false);
const [busy, setBusy] = useState(false);
const [error, setError] = useState<string | null>(null);

<ConfirmDialog
  open={open}
  onOpenChange={setOpen}
  tone="destructive"
  title="Delete this webhook endpoint?"
  description="The 3 attempts still queued against it are abandoned. This cannot be undone."
  confirmLabel="Delete endpoint"
  busy={busy}
  onConfirm={async () => {
    setBusy(true);
    const result = await deleteEndpoint(endpointId);
    setBusy(false);
    if (result.ok) setOpen(false);
    else setError(result.message);
  }}
>
  {error !== null && <p className="text-body text-state-danger-fg">{error}</p>}
</ConfirmDialog>
```

**The caller closes it, and that is the design.** `onConfirm` returning
does not close the dialog and `busy` is a prop rather than internal state,
so that a failed write leaves the dialog open with its context intact. A
self-closing confirm reports its failure through a toast, on a screen the
operator has already been returned to — which is where error messages go
to be missed.

---

## `Drawer` — a side panel that keeps the list

Generic parts: `Drawer`, `DrawerTrigger`, `DrawerContent`, `DrawerTitle`,
`DrawerDescription`, `DrawerClose`. Opinionated compositions:
`QuickDetailDrawer`, `MoreDetailDrawer`.

A drawer beats a dialog whenever the operator's place in a list is part of
the context — which, in a console, is most of the time. Reach for `Dialog`
when the decision is the whole task, and a drawer when the record is.

### The two opinionated ones

Both take the same props, and each has already made a decision for you:

```ts
interface DetailDrawerProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: ReactNode;                    // the drawer's accessible name — required
  description?: ReactNode;             // omitted, an sr-only fallback is still rendered
  children: ReactNode;                 // the scrollable body
  footer?: ReactNode;                  // a sticky action row
  className?: string;
}
```

- **`QuickDetailDrawer`** — a peek at one row. Narrow (`max-w-[440px]` at
  `md`+), **undimmed and non-modal**, so the list behind it stays legible,
  scrollable and clickable. Local state owns `open` (usually "which row id
  is selected"); losing it on refresh is expected, since reopening is one
  click. Read-mostly, one or two actions, nothing destructive.
- **`MoreDetailDrawer`** — the full record: every field, an edit form,
  destructive actions. Wide (`max-w-[680px]` at `md`+) and dimmed. The
  **caller is expected to own a shallow `?panel=<recordId>` route** so the
  panel survives a refresh and can be linked to; the component owns the
  weight, never the routing.

Below 768px both are a phone bottom sheet with M3's own shape: 28px top
corners (`rounded-t-sheet`, the `--radius-sheet` token — M3's corner for
sheets and dialogs only; do not put it on a card) and M3's 32×4 drag handle, with
22px above it and 22px below. That handle used to be a 5px bar 8px from
the top, so the title row now sits about 19px lower than it did — worth
knowing if a screenshot test pins it. Below 640px a `Select` inside one
opens as a second sheet over it with the same corner and handle — or, if
it has a `SelectSearch`, as the full-screen search view over the whole
screen, not just the drawer. Between 640 and 767px the drawer is a sheet
but a `SelectContent` is still a dropdown (a docked search view if
searchable), and a `SelectDropdown` is a dropdown at every width.

One limitation to know before you pick the quick one for a background
task: `modal={false}` in vaul 1.1.2 removes the dim and the pointer
blocking, but Radix's focus trap and `aria-modal="true"` underneath it are
unconditional. So an undimmed quick drawer is still keyboard-trapped and
still announced as modal. That is the library's limitation, verified in
vaul's own source, not a gap left open on purpose.

```tsx
const [selected, setSelected] = useState<string | null>(null);
const [confirming, setConfirming] = useState(false);

<MoreDetailDrawer
  open={selected !== null}
  onOpenChange={(next) => {
    if (!next) { setSelected(null); setConfirming(false); }
  }}
  title="msg_01J8ZQ4M7X"
  description="Everything recorded for this message."
  footer={
    <Button type="button" size="sm" variant="destructive" onClick={() => setConfirming(true)}>
      Cancel message
    </Button>
  }
>
  {confirming ? (
    <InlineConfirm
      title="Cancel this message?"
      description="It has not been submitted to orange_cm yet. Cancelling is final."
      confirmLabel="Cancel message"
      onConfirm={cancelMessage}
      onCancel={() => setConfirming(false)}
    />
  ) : (
    <DetailList>…</DetailList>
  )}
</MoreDetailDrawer>
```

**Never open a `Dialog` from inside an open drawer.** vaul's content
mounts a trapped, document-level focus scope regardless of `modal`, and
Headless UI's dialog always portals to its own sibling root — so the trap
pulls focus straight back out of the portal and the confirmation never
becomes visible. That failure is exactly why `InlineConfirm` exists.
Render it *instead of* the drawer's body (and drop the drawer's `footer`,
since `InlineConfirm` brings its own action row).

That includes `SideNav`'s `accountSlot`: below 1280px it renders in the
toolbar's More sheet, which is a vaul drawer. A `ConfirmDialog` opened
from Sign out there was measured opening under the sheet's scrim, where a
click or a tap cannot reach its buttons (`primitives-layout.md`,
"`accountSlot` below 1280px — the More sheet").

### The generic composition

For a one-off drawer with no quick-vs-more distinction to encode.
`DrawerTrigger` and `DrawerClose` are Radix-shaped here, not Headless
UI-shaped: they take **`asChild`**, not `as`.

```tsx
<Drawer direction="right">
  <DrawerTrigger asChild>
    <Button type="button" size="sm" variant="secondary">Open filters</Button>
  </DrawerTrigger>
  <DrawerContent className="p-5">
    <DrawerTitle>Filter deliveries</DrawerTitle>
    <DrawerDescription className="mt-1">
      Applies to the current provider only.
    </DrawerDescription>
    <div className="mt-auto flex justify-end pt-4">
      <DrawerClose asChild>
        <Button type="button" size="sm" variant="ghost">Close</Button>
      </DrawerClose>
    </div>
  </DrawerContent>
</Drawer>
```

`DrawerTitle` and `DrawerDescription` are not decoration — vaul renders
Radix's dialog content underneath, which warns in dev without a title, and
without a description `aria-describedby` points at nothing.

**Pass `direction="right"` if you keep `DrawerContent`'s default
placement.** `DrawerContent` is CSS-positioned at the right edge
(`inset-y-0 right-0`, `max-w-[560px]`), but vaul's `direction` prop
defaults to `"bottom"` and is what selects the open/close keyframes and
the drag axis. Left unset, a right-hand panel slides in vertically and
drags on the wrong axis. (This is a real mismatch: `drawer.tsx`'s
top-of-file comment claims the direction defaults to `"right"`, and vaul's
own types and the same file's later comment both say `"bottom"`.)

Do not render `DrawerPortal` or `DrawerOverlay` yourself — `DrawerContent`
renders both, and a second portal is the bug you get for trying.

**If you ever put `SideNav` inside a drawer:** vaul stamps
`will-change: transform` on the drawer, and that establishes a containing
block for `fixed` descendants. The nav's rails are portalled to
`document.body` precisely so this cannot reach them; if you hand-roll a
floating element, assume someone will wrap it in a drawer.

---

## `Popover` — anchored content next to its trigger

Parts: `Popover`, `PopoverTrigger`, `PopoverContent`.

Use it for a little more than a label: a filter form, a short explanation
with a link in it, a compact preview. Anything the reader might want to
click belongs here rather than in a `Tooltip`, which cannot hold
interactive content at all.

`PopoverTrigger` is polymorphic (`as={Button}`). `PopoverContent` defaults
to `anchor="bottom start"` and accepts Headless UI's full anchor value
(`"top end"`, an object with `gap`/`offset`, and so on). Because it is
anchored, the panel is **portalled** — which means, unlike `Tooltip`, a
scrolling ancestor cannot clip it.

```tsx
<Popover>
  <PopoverTrigger as={Button} type="button" variant="ghost" size="sm">
    Provider response
  </PopoverTrigger>
  <PopoverContent anchor="bottom end" className="max-w-[22rem]">
    <p className="text-body text-muted-foreground">
      orange_cm replied 409 insufficient_float at 14:02:11 UTC. The float was topped up
      at 14:40, so a requeue should now clear.
    </p>
  </PopoverContent>
</Popover>
```

---

## `DropdownMenu` — the actions on a row

Parts: `DropdownMenu`, `DropdownMenuTrigger`, `DropdownMenuContent`,
`DropdownMenuItem`, `DropdownMenuLinkItem`, `DropdownMenuCheckboxItem`,
`DropdownMenuGroup`, `DropdownMenuLabel`, `DropdownMenuSeparator`.

**`DropdownMenuItem` is a `<button>`; `DropdownMenuLinkItem` is an `<a>`.**
They render identically on purpose — the difference is only in what the
browser will let them do. A menu of *destinations* built from buttons
silently loses middle-click, ⌘-click, "open in new tab" and "copy link
address": four things people do with navigation without thinking, none of
which an `onClick` can be made to do, and none of which anyone files a bug
about. They just stop using the menu. **Commands get the button, places
get the link.**

```tsx
<DropdownMenu>
  <DropdownMenuTrigger as={Button} variant="secondary" size="sm">
    Actions
  </DropdownMenuTrigger>
  <DropdownMenuContent>
    <DropdownMenuLabel>This message</DropdownMenuLabel>
    <DropdownMenuItem onClick={() => replay(messageId)}>Replay</DropdownMenuItem>
    <DropdownMenuItem onClick={() => copy(messageId)}>Copy id</DropdownMenuItem>
    <DropdownMenuSeparator />
    <DropdownMenuLinkItem href={`/providers/orange_cm`}>
      Open provider
    </DropdownMenuLinkItem>
    <DropdownMenuLinkItem href={`/audit?message=${messageId}`} aria-current="page">
      Audit log
    </DropdownMenuLinkItem>
  </DropdownMenuContent>
</DropdownMenu>
```

`DropdownMenuCheckboxItem` is controlled by you — pass `checked`, handle
`onClick`. The menu stays open on click, which is what makes a column
toggle usable.

```tsx
<DropdownMenuGroup>
  {columns.map((key) => (
    <DropdownMenuCheckboxItem
      key={key}
      checked={visible[key]}
      onClick={() => setVisible((v) => ({ ...v, [key]: !v[key] }))}
    >
      {key}
    </DropdownMenuCheckboxItem>
  ))}
</DropdownMenuGroup>
```

**Its checked state lives in the accessible name, as an `sr-only`
", checked" after the label — not in `aria-checked`.** Headless UI's menu
item builds `role` into its own props and wins, so the element renders as
`role="menuitem"` no matter what is asked for; `aria-checked` is not
permitted on that role, so the earlier version was invalid ARIA that
reached assistive tech as nothing while reading in the source as handled.
Do not re-derive this and do not "fix" it back. It means a test should
match the row by regex (`{ name: /^cost/i }`), not by exact string.

Two more things worth knowing about the underlying menu: focus stays on
the menu container and moves an `aria-activedescendant` pointer, so
"which row is focused" is an attribute on the menu rather than
`document.activeElement`; and a long label is truncated one level in, with
the panel itself bounded at `max-w-[min(20rem,calc(100vw-2rem))]` so a
sentence-length item cannot make a menu wider than the screen edge leaves
room for.

`DropdownMenuLabel` is a plain non-interactive `<div>` — it deliberately
does not use Headless UI's heading, which throws unless it is inside a
section.

---

## `CommandMenu` — type-to-find

Parts: `CommandMenu`, `CommandMenuInput`, `CommandMenuList`,
`CommandMenuGroup`, `CommandMenuItem`, `CommandMenuEmpty`.

Built on cmdk, which owns the filtering, the keyboard navigation and the
ARIA. Note it is **not itself an overlay**: it renders in place, inside
whatever container you give it. Nothing here binds ⌘K and nothing here
opens it — the shortcut and the container are the application's decision.
Nothing in this package composes it into a `Dialog` either, so the ⌘K
palette shape is yours to wire and yours to check.

Useful props come straight from cmdk: `shouldFilter`, `filter`, `loop`,
`value`/`onValueChange` on `CommandMenu`; `value`, `keywords`, `onSelect`,
`disabled` on `CommandMenuItem`; `heading` on `CommandMenuGroup`.
`CommandMenuList` caps itself at `max-h-80` and scrolls.

```tsx
<CommandMenu loop>
  <CommandMenuInput placeholder="Search messages, routes, providers…" />
  <CommandMenuList>
    <CommandMenuEmpty>No results.</CommandMenuEmpty>
    <CommandMenuGroup heading="Go to">
      <CommandMenuItem onSelect={() => router.push("/messages")}>Messages</CommandMenuItem>
      <CommandMenuItem onSelect={() => router.push("/routes")}>Routes</CommandMenuItem>
      <CommandMenuItem
        keywords={["orange", "mtn", "provider"]}
        onSelect={() => router.push("/providers")}
      >
        Providers
      </CommandMenuItem>
    </CommandMenuGroup>
  </CommandMenuList>
</CommandMenu>
```

---

## `Tooltip` — one line of plain text

```ts
interface TooltipProps {
  label: string;                                  // plain text only
  position?: "top" | "bottom" | "left" | "right"; // default "top"
  className?: string;
  children: ReactNode;
}
```

No JS, no portal: this is daisyUI's CSS tooltip, drawn as a pseudo-element
from `content: attr(data-tip)`. That buys no second behaviour library and
costs two things.

- **The label must be a string.** No rich content, no links, no
  interactive tooltips anywhere. If you need one, that is a `Popover`.
- **Any scrolling ancestor clips the bubble.** A pseudo-element cannot
  escape an `overflow` boundary without a portal or CSS anchor
  positioning, and this component has neither. `Table`'s wrapper is such
  an ancestor, so a tooltip in a table cell hits this.

The component sets a native `title` with the same text on the same
element, so where the bubble is clipped the label degrades to the
browser's own tooltip rather than disappearing. That is a safety net, not
a plan: **inside a scroller, use a plain `title` attribute** and do not
design around the styled bubble. Where nothing clips it, both fire — the
styled one immediately, the native one after the browser's delay.

```tsx
<Tooltip label="Inferred from the MSISDN prefix — not authoritative." position="right">
  <span className="cursor-help text-body text-muted-foreground underline decoration-dotted">
    orange_cm
  </span>
</Tooltip>
```

It renders an `inline-block` wrapper `<div>` around `children`, so it does
not break a line, but it is a real element in your layout.

---

## Toasts — `toast`, `dismissToast`, `Toaster`

A function plus one mounted host. Call `toast(...)` or `dismissToast(...)`
from anywhere — an event handler, a mutation callback, a module with no
React in it — and render `<Toaster />` **once**, near the app root.

```ts
toast(item: {
  title: string;
  description?: string;
  variant?: "default" | "success" | "danger";
  durationMs?: number;    // default 4000; 0 means it never expires on its own
}): string                // the id

dismissToast(id: string): void
```

```tsx
// once, near the root
<Toaster />

// anywhere
const id = toast({
  title: "Payout requeued",
  description: "pay_01J8ZQ4M7X submitted to orange_cm.",
  variant: "success",
});

// and, if the write turns out to have failed after all
dismissToast(id);
toast({
  title: "Requeue failed",
  description: 'missing permission "payout:requeue"',
  variant: "danger",
  durationMs: 0,
});
```

What the host does, so you do not work around it:

- **The stack is capped at four, oldest dropped.** Before the cap, ten
  rapid calls stacked ten cards — 600px of viewport with no ceiling. Past
  four, nobody reads them anyway.
- The title clamps to two lines and the description to three, in a fixed
  `w-80` column pinned bottom-right. Anything that needs more room is not
  a toast; put it inline, in an `InlineBanner`.
- There is no enter/exit transition today — a card appears and vanishes in
  one frame, and survivors jump upward when one above them expires.
- The live region is the container and only the container
  (`aria-live="polite"`, `aria-atomic="false"`). Do not add `role="status"`
  to your own toast content: a nested live region shadows the one that can
  actually announce, and an atomic region re-reads the whole stack on
  every change ("Copied. Copied. Copied.").

---

## `InlineConfirm` — the confirmation that is not an overlay

Rendered in the caller's own DOM subtree. No portal, no focus scope, no
overlay, no open/close transition of its own — which is precisely why it
works inside a drawer where a nested `Dialog` does not.

```ts
interface InlineConfirmProps {
  title: ReactNode;              // the action being taken, not the record
  description?: ReactNode;
  children?: ReactNode;          // a field belonging to the confirmation, e.g. a Select
  confirmLabel: string;
  pendingLabel?: string | undefined;   // default: confirmLabel + "…"
  cancelLabel?: string;                // default "Cancel"
  onConfirm: () => void;
  onCancel: () => void;
  pending?: boolean;             // both buttons disabled, confirm shows pendingLabel
  confirmDisabled?: boolean;     // independent of pending — e.g. nothing picked yet
  destructive?: boolean;         // default true: danger button and accent border
  error?: ReactNode;             // rendered as an InlineBanner above the buttons
  className?: string;
}
```

The caller owns visibility: render it **instead of** the body it belongs
to, not layered on top. It covers both shapes the console needs — a plain
yes/no (`title`/`description`, no `children`) and a short form embedded in
a confirmation step (`children` holds the field). Pass
`destructive={false}` for the second kind when nothing is being destroyed.

```tsx
<InlineConfirm
  title="Register 237600000000 with a provider"
  description="The number starts receiving traffic as soon as the provider accepts it."
  destructive={false}
  confirmLabel="Register"
  pendingLabel="Registering…"
  confirmDisabled={provider === null}
  pending={pending}
  error={error}
  onConfirm={register}
  onCancel={() => setConfirming(false)}
>
  <Select value={provider} onChange={setProvider} …/>
</InlineConfirm>
```
