# Setup

## 1. Install

```sh
pnpm add @vaam-apps/ui
```

Peer dependencies you must already have: `react` (^18.3 || ^19),
`react-dom`, `tailwindcss` ^4.1, `daisyui` ^5.

## 2. The stylesheet

In your global CSS, **after** Tailwind and the daisyUI plugin:

```css
@import "tailwindcss";
@plugin "daisyui" {
  themes: false;
}
@import "@vaam-apps/ui/styles/theme.css";
@source "../node_modules/@vaam-apps/ui/dist";
```

Adjust the `@source` path to reach your own `node_modules`.

### `themes: false` is load-bearing

This package registers its themes under daisyUI's own `dark` and `light`
names, and daisyUI's built-ins emit at a higher specificity than any
custom theme block:

```css
/* daisyUI's built-in — specificity (0,3,1) */
:is(:root:has(input.theme-controller[value=dark]:checked),[data-theme=dark]) { … }
/* a custom theme block — specificity (0,1,0) */
:where(:root),[data-theme=dark] { … }
```

The built-in wins on every token it also defines — `base-100`,
`base-200`, `base-300`, `base-content` — regardless of import order. The
symptom is a page that looks *almost* right: stock daisyUI greys instead
of this theme's indigo-tinted M3 surfaces. Measured once on a real page:
`--color-base-100` resolved to `oklch(25.33% .016 252.42)` where the
theme declared its own surface.

### `@source` is load-bearing

Tailwind v4 generates only the utilities it can see used, and it does not
scan `node_modules` on its own. Omit this and **every component renders
completely unstyled** — no error, no warning, just a page that looks
broken in a way that is miserable to debug.

## 3. Theme attribute — optional

`dark` is daisyUI's `default: true` theme here and the custom properties
are declared at `:root` as well, so a page with **no `data-theme`
attribute at all** gets the dark theme correctly. Set
`<html data-theme="light">` to opt into light.

One upgrade caveat: if your `<html>` already carries
`data-theme="light"` from daisyUI boilerplate, that attribute used to
match nothing and now matches. Set `data-theme="dark"` explicitly if that
is you.

To avoid a flash of the wrong theme on first paint, render
`themeInitScript` in `<head>` before any content:

```tsx
import { themeInitScript } from "@vaam-apps/ui";

<script
  // biome-ignore lint/security/noDangerouslySetInnerHtml: a fixed literal
  dangerouslySetInnerHTML={{ __html: themeInitScript }}
/>
```

It is a single constant string with nothing interpolated into it — keep
it that way; building it from variables is what a code scanner flags.

## 4. Fonts — optional, and the failure is quiet rather than broken

`theme.css` names four roles as **stacks, not `@font-face` rules**, and
the package ships no font files:

- `--font-display` — Roboto Flex (M3's *brand* face)
- `--font-sans` — Roboto Flex (M3's *plain* face)
- `--font-italic` — Roboto Flex, drawn on its slant axis by `italic`
- `--font-mono` — JetBrains Mono (data: ids, money, timestamps)

Load them however you already load fonts:

```html
<link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=Roboto+Flex:opsz,slnt,wdth,wght@8..144,-10..0,25..151,100..1000&family=JetBrains+Mono:wght@400;500&display=swap">
```

Skipping this is not a failure: the stacks fall back through `system-ui` /
`ui-monospace` and every screen still works at every role's size and
weight — it just stops looking like M3.

## 5. Reading the Material 3 Expressive tokens

Everything is a custom property, generated or transcribed from Material's
own sources, and readable from any stylesheet or inline style:

- **Colour** — `--md-sys-color-<role>` for every M3 role (`primary`,
  `on-primary`, `primary-container`, `on-primary-container`, `surface`,
  `surface-container-low` … `surface-container-highest`, `on-surface`,
  `on-surface-variant`, `outline`, `outline-variant`, `inverse-surface`,
  `error`, `scrim`, …), dark and light, from the seed `#4f46e5`. The status
  hues are M3 custom colours: `--md-custom-color-<hue>`, `-on-<hue>`,
  `-<hue>-container`, `-on-<hue>-container`.
- **Shape** — `--md-sys-shape-corner-{none, extra-small (4px), small (8),
  medium (12), large (16), large-increased (20), extra-large (28),
  extra-large-increased (32), extra-extra-large (48), full}`. Tailwind's
  `rounded-xs` … `rounded-4xl` are that scale in order.
- **Type** — `--md-sys-typescale-<role>-{font,size,line-height,tracking,weight}`
  for the fifteen roles (`display-large` … `label-small`) and
  `--md-sys-typescale-emphasized-<role>-weight`. Utilities:
  `text-body-medium`, `text-title-large`, `text-headline-small`, …; the
  library's own `text-body`, `text-caption`, `text-title` and friends are
  aliases of M3 roles now.
- **Motion** — `--md-sys-motion-spring-{spatial,effects}-{fast,default,slow}`
  and `…-duration`: the Expressive springs as `linear()` easings.

```css
.callout {
  background: var(--md-sys-color-primary-container);
  color: var(--md-sys-color-on-primary-container);
  border-radius: var(--md-sys-shape-corner-large);
}
```

Overriding a `--md-sys-color-*` role re-themes every component that reads
it (daisyUI's tokens and this library's own are mappings onto the roles).
A different seed is a full regenerated set declared on `:root` and
`[data-theme="light"]` after `theme.css` — and it is not covered by the
package's contrast and status-separation gates, so measure `primary`
against `success` and `progress` before shipping one.

## 6. Verify before building a screen on it

Render one `Button` and one `StatusPill` and check in a browser that the
page background is a near-black indigo, the button is a light indigo
stadium, and the pill has a coloured glyph. Both
setup failures above produce a page that *renders* — they just render
wrong, and they are far cheaper to find now than inside a finished screen.
