---
name: vaam-ui
description: Build operator-console screens with @vaam-apps/ui — the dark-first React component library used by vpay and vsms. Use when installing or configuring the package, wiring Tailwind v4 + daisyUI for it, declaring a status system, choosing a component, or debugging a screen that renders unstyled, uncoloured, or with the wrong surface.
---

# @vaam-apps/ui

A dark-first React component library for operator consoles — screens where
someone is answering *"what happened to this one record, and does anybody
need to do something?"* about one row out of a hundred thousand.

Peer dependencies: `react` ^18.3 || ^19, `react-dom`, `tailwindcss` ^4.1,
`daisyui` ^5.

## The one idea everything else follows from

**Colour means something here, and the status system owns it.** A state's
presentation is declared once, as data, and several components render it.
The alternative — each call site picking a colour and a word — is how the
same failure shows up as an amber chip on one screen, a red badge on
another, and the word "error" on a third, until a reader stops trusting
the console's colours and starts reading every label.

So: **never hardcode a colour for a state.** Declare it in a status
system (`references/status-system.md`) and let `StatusPill` / `StateChip`
/ `StateMark` render it. Decorative colour — the aurora glow and mesh —
deliberately carries no state and cannot be tinted, which is the only
thing that keeps a coloured surface safe in a system like this.

## Setup, and the two ways it fails silently

```css
@import "tailwindcss";
@plugin "daisyui" {
  themes: false;
}
@import "@vaam-apps/ui/styles/theme.css";
@source "../node_modules/@vaam-apps/ui/dist";
```

Both lines that look optional are not:

- **`themes: false`** — daisyUI's built-in themes emit at a higher
  specificity than any custom theme block, so leaving them on means the
  page background, card surfaces and body text silently come out stock
  daisyUI rather than this theme.
- **`@source`** — Tailwind v4 generates only the utilities it can see and
  does not look inside `node_modules`. Without it, **every component
  renders with no styling at all** — no error, no warning.

If a screen looks broken, check those two first. Full detail and the
font/theme steps: `references/setup.md`.

## Finding the component you need

Every public export is documented, and a test in the package fails if one
is not — so if something is missing here, it does not exist.

| Reference | What is in it |
|---|---|
| `references/components.md` | How to choose: the three surface registers, and what belongs where |
| `references/primitives-input.md` | Buttons, text inputs, selects, checkboxes, switches, radios, chips, date pickers, `FormField` |
| `references/primitives-overlay.md` | Dialogs, drawers, popovers, dropdown and command menus, tooltips, toasts |
| `references/primitives-layout.md` | Cards, tables, tabs, pagination, `SideNav`, screen scaffolding, skeletons, theming |
| `references/data-display.md` | Ids, phones, money, timestamps, masked secrets, detail lists, stat tiles, `InstrumentPanel` |
| `references/patterns.md` | Banners, empty states, live rows, payload inspectors, timelines |
| `references/status-system.md` | `defineStatusSystem`, `StatusPill`, `StateChip`, `StateMark` |
| `references/utilities.md` | **`cn()`** — read this before writing a `className` — and `useReducedMotion` |
| `references/setup.md` | Install, the stylesheet, fonts, the theme attribute |
| `references/pitfalls.md` | Every entry is a bug that actually shipped |

Three orientation rules:

- **Surfaces come in three registers** — diagnostic (a hairline; most of
  the library), floating (a shadow, because it overlaps a ground it does
  not know), instrument (`InstrumentPanel` / `Card glow`, for data you
  *scan* rather than read).
- **`cn()` is exported** and is the only correct way to merge classes onto
  these components — plain string concatenation loses to `tailwind-merge`
  in ways that delete classes silently. `references/utilities.md` has the
  examples and the two bugs that forced its custom configuration.
- **Don't reach past the API for a colour.** The tokens are
  `bg-surface-*`, `border-edge*`, `text-foreground` /
  `text-muted-foreground` / `text-subtle-foreground`. An undeclared token
  generates nothing and fails invisibly.

## Before you ship a screen

`references/pitfalls.md` is short and every entry is a bug that actually
shipped. The ones that bite integrators most:

- `SideNav`'s rails are `fixed` and portalled to `document.body`, so they
  **cannot reserve their own space** — the content column's padding is
  yours to set.
- `text-subtle-foreground` is banned on the aurora mesh (it falls below
  AA there); `InstrumentPanel` already steps its own caption up.
- A tooltip inside a scrolling ancestor is clipped. Use a native `title`.

## Where the real documentation is

The published Storybook is the reference — every component has stories,
and `Docs/` carries long-form pages on theming, the status system,
surfaces, navigation, accessibility and testing. Read those for anything
this skill summarises.
