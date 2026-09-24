# Pitfalls

Every entry here is a bug that actually shipped, in this library or in a
consumer of it. They share a shape: the code looks right, nothing errors,
and the failure is visible only in a render.

## The page renders with no styling at all

You are missing `@source "../node_modules/@vaam-apps/ui/dist"`. Tailwind
v4 generates only what it can see and does not scan `node_modules`. No
error, no warning.

## The colours are *almost* right

You left daisyUI's built-in themes on. Use `@plugin "daisyui" { themes:
false; }` — the built-ins outrank any custom theme block on `base-100`,
`base-200`, `base-300` and `base-content`, so the page background and
card surfaces come out stock daisyUI.

## A token I used renders as nothing

Tailwind emits nothing for an unknown token and says nothing about it, so
`bg-surface-4` or `text-subtle` silently produce a transparent surface or
uncoloured text. The real names:

- surfaces: `bg-base-100` / `bg-base-200` / `bg-base-300`, `bg-surface-1`
  … `bg-surface-3`
- edges: `border-edge-subtle`, `border-edge`, `border-edge-strong`
- text: `text-foreground`, `text-muted-foreground`, `text-subtle-foreground`
- other: `bg-scrim`, `ring-ring`

## My class didn't apply to a component

Two causes, both real:

1. **Use `cn()`.** Plain concatenation puts your class in a string
   `tailwind-merge` then resolves — and it once deleted `btn-circle`
   because it conflated shape with size.
2. **daisyUI emits into nested cascade layers**, and an *unlayered*
   Tailwind utility outranks a nested sublayer. If a daisyUI component
   class (`.btn-circle`, and similar) appears not to apply, this is why.

## Text over an `InstrumentPanel` or a glowing card is hard to read

`text-subtle-foreground` is banned on the aurora mesh — measured, it falls
to 4.41:1 in dark and 4.45:1 in light, below the 4.5:1 AA bar, while
`text-muted-foreground` holds at 5.29:1 and above. `InstrumentPanel`
already steps its own caption up; do not push it back down.

## My content sits under the nav rail

`SideNav`'s rails are `position: fixed` and portalled to `document.body`,
so they **cannot reserve their own space** — that is the trade that lets
one layout serve every width. The content column's padding is yours:
leave room on the left where the vertical rail floats, and at the bottom
where the phone pill does. Both are M3's floating toolbar now and each
ends 80px from its edge, so the numbers are `sm:pl-24` and `pb-24` —
`sm:pl-20`, which was right for the old 52px rail, now leaves content
flush against the toolbar. With `viewport-fit=cover` the bottom one also
rises by `env(safe-area-inset-bottom)`; add it to your `pb-*` too.

## A tooltip is invisible or cut in half

`Tooltip`'s bubble is a CSS pseudo-element, so **any scrolling ancestor
clips it**. Escaping one needs a portal or CSS anchor positioning and this
component has neither. Inside a scroller, use a native `title` instead —
the browser paints it outside the page entirely. `Tooltip` sets one as a
fallback, so the label is degraded rather than lost, but do not design
around the styled bubble there.

## A `sticky` table header doesn't stick

`sticky` needs a scrollport, and a page-level scroll is not one. Pass
`maxHeight` to `Table` to bound the wrapper. Unbounded, the header's
top moves 1:1 with the page — measured 72 → −228 on a 300px scroll.

## I wrapped SideNav in a drawer and the rail jumped

`transform`, `filter`, `backdrop-filter`, `contain` and
`will-change: transform` each establish a containing block for `fixed`
descendants — and `vaul` stamps `will-change: transform` on every drawer.
The rails are portalled to `document.body` precisely so this cannot
happen; if you re-implement a floating element yourself, assume a
consumer wrapped you in a drawer.

## Anything I animate ignores the user's motion preference

Use the exported `useReducedMotion()` rather than reading `matchMedia`
once during render — a one-shot read is frozen at mount and never sees the
preference change.
