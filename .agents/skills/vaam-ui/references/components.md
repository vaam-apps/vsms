# Choosing a component

The per-component detail lives in the sibling references — this page is
the question you ask *before* you open one: which kind of thing am I
building, and what does this system already have an opinion about?

## Surfaces come in three registers

Picking the wrong one is the most common taste error, and it is the one a
reviewer will notice immediately.

- **Diagnostic** — `Card`, `Table`, `DetailList`, the drawers. A hairline
  on a surface step, and nothing else competing. You *read* these, row by
  row, looking for the one that is wrong. **This is most of the library
  and should be most of your screens.**
- **Floating** — `Dialog`, `Drawer`, `Popover`, `Toaster`. These get a
  shadow, because they overlap a ground they do not know.
- **Instrument** — `InstrumentPanel`, and `Card` with `glow`. An aurora
  mesh ground for data you *scan* rather than read: a row of metrics, a
  headline figure and its denominator. Use it sparingly — a screen where
  every card glows has no glow.

The registers are a vocabulary, not a hierarchy. An instrument panel is
not a "better" card; it answers a different question, and using it for a
list of records makes the list harder to read.

## Colour is not yours to pick

The status system owns it. A state's hue, shape and label are declared
once as data and rendered by `StatusPill` / `StateChip` / `StateMark` —
see `status-system.md`. Do not reach for a Tailwind colour to say
"failed": the whole point is that one failure looks the same on every
screen in the console.

The decorative colour — the aurora glow and mesh — deliberately carries
no state, cannot be tinted per state, and is bound to the same hue ramp
so it cannot drift into looking like a signal. That is the only reason a
coloured surface is safe in a system whose premise is that colour means
something.

## Which reference

| You are building | Read |
|---|---|
| A form, a filter bar, anything the operator types into | `primitives-input.md` |
| Something that opens over the page | `primitives-overlay.md` |
| The page itself — tables, cards, tabs, navigation, theming | `primitives-layout.md` |
| A record's values: ids, money, phones, timestamps, secrets | `data-display.md` |
| A banner, an empty state, a live-updating row, a timeline | `patterns.md` |
| A state machine's presentation | `status-system.md` |
| Anything with a `className` on it | `utilities.md` |

## Two rules that apply everywhere

**Use `cn()` to merge classes.** Not a template string, not `clsx`. It
resolves Tailwind conflicts in call order and knows this package's custom
font sizes, which plain `tailwind-merge` mistakes for colours.
`utilities.md` has the examples.

**Use the tokens, not raw colours.** `bg-surface-*`, `border-edge*`,
`text-foreground` / `text-muted-foreground` / `text-subtle-foreground`.
Tailwind generates nothing for a token that does not exist and says
nothing about it, so a typo is a transparent surface rather than an
error.
