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
of this theme's near-blacks. Measured once on a real page:
`--color-base-100` resolved to `oklch(25.33% .016 252.42)` where the
theme declares `#0a0b0d`.

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

- `--font-display` — IBM Plex Serif
- `--font-sans` — IBM Plex Sans
- `--font-italic` — IBM Plex Serif
- `--font-mono` — JetBrains Mono

Load them however you already load fonts:

```html
<link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=IBM+Plex+Sans:ital,wght@0,400;0,500;0,600;1,400&family=IBM+Plex+Serif:ital,wght@0,500;0,600;1,400;1,500&family=JetBrains+Mono:wght@400;500&display=swap">
```

Skipping this is not a failure: the stacks fall back through `ui-serif` /
`system-ui` / `ui-monospace` and every screen still works. What is lost is
quieter — the display and italic roles collapse into the same system
serif, and the four-voice distinction goes with them.

## 5. Verify before building a screen on it

Render one `Button` and one `StatusPill` and check in a browser that the
page background is near-black and the pill has a coloured glyph. Both
setup failures above produce a page that *renders* — they just render
wrong, and they are far cheaper to find now than inside a finished screen.
