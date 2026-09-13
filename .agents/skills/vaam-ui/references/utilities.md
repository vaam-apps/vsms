# Utilities

Two exports, both small, both about a failure you cannot see in the source:
a class that was deleted on the way out, and an animation that ignores the
person watching it.

## `cn()`

```ts
import { cn } from "@vaam-apps/ui";

cn(...inputs: ClassValue[]): string
```

`clsx` for the conditionals, then a `tailwind-merge` instance taught about
this package's theme scales and daisyUI's component modifiers. **It is the
only correct way to put a class on one of these components.**

### Why not a template string

Every component in this package merges internally as `cn(base, className)`,
so a class you pass wins over the component's own — *in call order*, which
is the order you can see. Join the strings yourself and both survive into
the DOM, where the winner falls out of stylesheet order instead: two
utilities of equal specificity are decided by whichever Tailwind happened to
emit last, which is not something the call site controls.

```tsx
// Wrong: both classes reach the DOM, and which one paints is Tailwind's
// emission order, not yours.
function AmountCell({ className }: { className?: string }) {
  return <td className={`px-3 py-2 text-right ${className ?? ""}`}>…</td>;
}
<AmountCell className="px-6" />   // px-3 or px-6 — you find out in a browser

// Right: `px-3` is gone before the markup exists.
function AmountCell({ className }: { className?: string }) {
  return <td className={cn("px-3 py-2 text-right", className)}>…</td>;
}
<AmountCell className="px-6" />   // "py-2 text-right px-6"
```

`clsx` alone has the same problem: it joins, it does not resolve. And in
this package stylesheet order is worse than a coin flip, because daisyUI
emits into nested cascade layers and an *unlayered* Tailwind utility
outranks a daisyUI sublayer — an icon button's own `rounded-field` beat
`.btn-circle`, so a control everything on the page called a circle shipped
as a rounded square. Resolving in call order is how you keep that argument
out of the browser.

### Conditionals

Everything `clsx` accepts passes through — `false`, `undefined`, arrays,
objects — and only then gets merged:

```ts
cn("a", false && "b", undefined, ["c", { d: true, e: false }]); // "a c d"
```

Which in a real row looks like this. Note that the later class wins, so the
conditional goes last:

```tsx
<TableRow
  className={cn(
    "cursor-pointer",
    payout.state === "failed" && "bg-state-danger-bg",
    selectedId === payout.id && "bg-surface-3",
  )}
>
```

### The two custom groups, and the bugs they fix

Stock `tailwind-merge` knows Tailwind's *default* theme and Tailwind's own
utilities. Both gaps produced the same failure here — a class that parses,
looks right in the source, and is silently dropped — and both were found by
checking merge output rather than by reading documentation.

**1. Size and shape are two groups.** `btn-square` / `btn-circle` /
`btn-wide` / `btn-block` once shared a group with `btn-xs … btn-xl`, so they
were treated as mutually exclusive with a size. They are not: daisyUI's
`.btn-circle` sets width and height from `--size` and overrides the radius,
while `.btn-sm` sets `--size` itself. Collapsed into one group, the shape
was deleted:

```ts
// Before: the shape vanished, and `Button size="icon"` — which emits
// exactly this pair — never rendered as a circle.
cn("btn-circle", "btn-sm"); // "btn-sm"

// Now:
cn("btn-circle", "btn-sm"); // "btn-circle btn-sm"
cn("btn-sm", "btn-circle"); // "btn-sm btn-circle"

// Two shapes, or two sizes, still resolve against each other:
cn("btn-square", "btn-circle"); // "btn-circle"
cn("btn-sm", "btn-lg");         // "btn-lg"
```

The same split exists for colour versus style, for the same reason, and it
is the counter-intuitive one: daisyUI 5's `.btn-outline` *reads* the
`--btn-color` variable that `.btn-primary` sets, so `btn-primary btn-outline`
is how you spell "a primary outline button".

```ts
cn("btn btn-primary", "btn-outline"); // "btn btn-primary btn-outline" — kept
cn("btn btn-primary", "btn-ghost");   // "btn btn-ghost" — two colours, last wins
cn("btn-primary-ish", "btn-primary"); // both — an unrelated class is not a modifier
```

**2. This package's font sizes are registered as font sizes.** The
`--text-*` steps in `theme.css` — `micro`, `caption`, `body`, `prose`,
`title-sm`, `title`, `metric`, `metric-lg` — are theme keys stock
`tailwind-merge` has never heard of, so it fell back to classifying
`text-caption` as a *colour* and dropped it as conflicting with the real
colour beside it:

```ts
// Stock tailwind-merge, on strings taken verbatim from `FieldError`,
// `DetailList` and `CardHeader`:
twMerge("text-caption text-state-danger-fg"); // "text-state-danger-fg"
twMerge("text-body text-foreground");         // "text-foreground"
twMerge("text-title-sm text-foreground");     // "text-foreground"
```

Every one of those had been rendering at the browser's default font size,
in this package and in anything consuming it, for as long as `cn()` had
existed — and tailwind-merge 2.6.0 was checked directly, so this was a
latent bug rather than a version regression. Registering the scale under
the `text` theme key separates the two groups again. The registered list is
exported as `REGISTERED_FONT_SIZES`, not as API but so a test can check it
against the stylesheet rather than trust the list.

```ts
cn("text-caption text-state-danger-fg"); // unchanged — size and colour coexist
cn("font-mono text-caption text-subtle-foreground"); // unchanged
cn("text-metric", "text-micro");         // "text-micro" — two sizes still collapse
cn("text-foreground", "text-muted-foreground"); // "text-muted-foreground"
```

daisyUI's radius tiers got the same treatment, for the same reason — they
did not conflict with Tailwind's own at all, so `rounded-sm rounded-field`
kept both classes and let the stylesheet pick:

```ts
cn("rounded-sm", "rounded-field");  // "rounded-field"
cn("rounded-box", "rounded-none");  // "rounded-none"
```

### Two things worth knowing before you debug a merge

- **Groups can be wider than they look.** `transition-colors` and
  `transition-opacity` are the *same* group, because both set
  `transition-property` — measured, not assumed: `cn("transition-colors",
  "transition-opacity")` is `"transition-opacity"`. `CopyButton` uses the
  bare `transition` utility precisely because it animates both.
- **`cn()`'s configuration is closed.** It knows *this* package's scales. If
  your application declares its own `--text-*` steps, `tailwind-merge` will
  mistake them for colours in exactly the way described above, and you need
  your own `extendTailwindMerge` for them — `cn()` will not learn them.

## `useReducedMotion()`

```ts
import { useReducedMotion } from "@vaam-apps/ui";

useReducedMotion(): boolean
```

`true` when the visitor has asked for reduced motion. A subscription over
`matchMedia("(prefers-reduced-motion: reduce)")` via React's useSyncExternalStore,
not a read.

**Why that matters:** a bare `matchMedia(query).matches` evaluated during
render is frozen at mount. A preference changed while the page is open — an
operator toggling it in the OS settings panel, or Storybook's own
reduced-motion toolbar, which flips the query live rather than reloading —
never reaches the component. `LiveRow` had exactly that bug.

Server-safe: `getServerSnapshot` returns `false`, motion allowed, because
the server cannot know the setting. The cost is one extra frame of animation
on a reduced-motion visitor's first paint before the post-hydration re-check
corrects it — cosmetic and one-time, which is why there is no blocking init
script for this the way there is for the theme. If `matchMedia` is missing
or blocked, it also fails open to `false` rather than throwing out of a
render.

```tsx
function PayoutFlash({ washed }: { washed: boolean }) {
  const reducedMotion = useReducedMotion();

  return (
    <div
      className={cn(
        // The transition classes stay unconditional — gate the *tint*, not
        // the transition, or the fade-out lands on a style with nothing to
        // interpolate and the colour simply blinks off.
        reducedMotion ? "transition-none" : "transition-colors ease-out",
        washed && "bg-state-success-fg/10",
      )}
    >
      …
    </div>
  );
}
```

The rule the library follows, and the one to follow in your own surfaces:
under reduced motion the **signal survives and only the animation stops**.
`LiveRow` turns its 240ms wash-and-decay into a static 1200ms hold rather
than skipping the wash; `Skeleton` goes still rather than disappearing.
Removing the feedback entirely is not what the preference asks for.
