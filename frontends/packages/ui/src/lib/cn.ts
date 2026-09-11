import { type ClassValue, clsx } from "clsx";
import { extendTailwindMerge } from "tailwind-merge";

/**
 * `tailwind-merge` taught about (a) this design system's own theme scales
 * and (b) daisyUI's component modifiers.
 *
 * Stock `twMerge` only knows Tailwind's *default* theme and Tailwind's own
 * utilities. Both gaps produce the same failure shape — a class that still
 * parses, still looks plausible in the source, and quietly does nothing —
 * and both were found by probing real class strings from this package
 * rather than by reading the docs:
 *
 * **1. Custom font sizes were being deleted.** `--text-caption` &c. are
 * theme keys stock `twMerge` has never heard of, so it classified
 * `text-caption` by its fallback rule — as a *colour* — and then treated
 * it as conflicting with the actual colour beside it:
 *
 * ```text
 * twMerge("text-caption text-state-danger-fg")  // → "text-state-danger-fg"
 * twMerge("text-body text-foreground")          // → "text-foreground"
 * twMerge("text-title-sm text-foreground")      // → "text-foreground"
 * ```
 *
 * Those three strings are, verbatim, `FieldError`, `DetailList` and
 * `CardHeader`. Every one of them has been rendering at the browser's
 * default font size rather than its intended step, in both this package
 * and any app consuming it, since `cn()` was written. Registering the
 * scale under the `text` theme key is what separates the two groups again.
 * This predates the tailwind-merge 3.x upgrade — v2.6.0 was checked
 * directly and drops the same classes — so it is a latent bug being fixed
 * here, not a regression being papered over.
 *
 * **2. Radius utilities did not conflict at all.** `rounded-field` /
 * `rounded-box` / `rounded-selector` are daisyUI's own radius tiers, and
 * stock `twMerge` left `rounded-sm rounded-field` as *both* classes, so
 * the winner fell out of stylesheet order rather than call order — the
 * opposite of what an override prop is for.
 *
 * **3. daisyUI component modifiers did not conflict either**, so
 * `cn("btn btn-primary", "btn-ghost")` kept both. The group list below is
 * adapted from the sibling `@vpay/ui` package, which had already derived
 * it against daisyUI 5's compiled CSS; its central finding is preserved
 * and worth restating, because it is counter-intuitive:
 *
 * > **Colour and style are two groups, not one, for `btn` and `badge`.**
 * > `.btn-outline`/`.btn-dash` *read* the `--btn-color` variable that
 * > `.btn-primary` sets, so `btn-primary btn-outline` is how daisyUI 5
 * > spells "a primary outline button". Collapsed into one group,
 * > `cn("btn-primary", "btn-outline")` returns `btn-outline` and the
 * > colour silently vanishes.
 *
 * `ghost` and `link` stay with the *colours* rather than the styles,
 * matching `@vpay/ui` and this package's own `buttonVariants`, where
 * `ghost` is offered as an alternative to `primary` rather than a
 * modifier on top of it.
 *
 * Scoped to what this package actually emits plus the daisyUI components a
 * consumer is most likely to reach for directly. Add a group when a
 * component gains a variant dimension, not ahead of one.
 */
type ExtraClassGroupId =
  | "daisy-btn-variant"
  | "daisy-btn-style"
  | "daisy-btn-size"
  | "daisy-badge-variant"
  | "daisy-badge-style"
  | "daisy-badge-size"
  | "daisy-alert-variant"
  | "daisy-alert-style"
  | "daisy-input-variant"
  | "daisy-select-variant"
  | "daisy-select-size"
  | "daisy-checkbox-variant"
  | "daisy-checkbox-size"
  | "daisy-radio-variant"
  | "daisy-radio-size"
  | "daisy-toggle-variant"
  | "daisy-toggle-size"
  | "daisy-card-size"
  | "daisy-table-size"
  | "daisy-tabs-style"
  | "daisy-tabs-size"
  | "daisy-progress-variant"
  | "daisy-loading-type"
  | "daisy-loading-size";

/**
 * The `--text-*` steps declared in `styles/theme.css`. Kept in sync with
 * that file by `theme-tokens.test.ts`, which parses the stylesheet rather
 * than trusting this list.
 */
const FONT_SIZES = [
  "micro",
  "caption",
  "body",
  "prose",
  "title-sm",
  "title",
  "metric",
  "metric-lg",
] as const;

/** daisyUI's radius tiers, which its own components read from. */
const RADII = ["selector", "field", "box"] as const;

const DAISY_COLOURS = [
  "primary",
  "secondary",
  "accent",
  "neutral",
  "info",
  "success",
  "warning",
  "error",
] as const;

const DAISY_SIZES = ["xs", "sm", "md", "lg", "xl"] as const;

const twMerge = extendTailwindMerge<ExtraClassGroupId>({
  extend: {
    theme: {
      // Registering these as *font sizes* is what stops `text-caption`
      // being mistaken for a colour and dropped. See point 1 above.
      text: [...FONT_SIZES],
      radius: [...RADII],
    },
    classGroups: {
      "daisy-btn-variant": [{ btn: [...DAISY_COLOURS, "ghost", "link"] }],
      "daisy-btn-style": [{ btn: ["outline", "soft", "dash"] }],
      "daisy-btn-size": [{ btn: [...DAISY_SIZES, "wide", "block", "square", "circle"] }],
      "daisy-badge-variant": [{ badge: [...DAISY_COLOURS, "ghost"] }],
      "daisy-badge-style": [{ badge: ["outline", "soft", "dash"] }],
      "daisy-badge-size": [{ badge: [...DAISY_SIZES] }],
      "daisy-alert-variant": [{ alert: ["info", "success", "warning", "error"] }],
      "daisy-alert-style": [{ alert: ["outline", "soft", "dash"] }],
      "daisy-input-variant": [{ input: [...DAISY_COLOURS, "ghost"] }],
      "daisy-select-variant": [{ select: [...DAISY_COLOURS, "ghost"] }],
      "daisy-select-size": [{ select: [...DAISY_SIZES] }],
      "daisy-checkbox-variant": [{ checkbox: [...DAISY_COLOURS] }],
      "daisy-checkbox-size": [{ checkbox: [...DAISY_SIZES] }],
      "daisy-radio-variant": [{ radio: [...DAISY_COLOURS] }],
      "daisy-radio-size": [{ radio: [...DAISY_SIZES] }],
      "daisy-toggle-variant": [{ toggle: [...DAISY_COLOURS] }],
      "daisy-toggle-size": [{ toggle: [...DAISY_SIZES] }],
      "daisy-card-size": [{ card: [...DAISY_SIZES] }],
      "daisy-table-size": [{ table: [...DAISY_SIZES] }],
      "daisy-tabs-style": [{ tabs: ["box", "border", "lift"] }],
      "daisy-tabs-size": [{ tabs: [...DAISY_SIZES] }],
      "daisy-progress-variant": [{ progress: [...DAISY_COLOURS] }],
      "daisy-loading-type": [{ loading: ["spinner", "dots", "ring", "ball", "bars", "infinity"] }],
      "daisy-loading-size": [{ loading: [...DAISY_SIZES] }],
    },
  },
});

/**
 * Merge class names, resolving conflicting utilities so the *last* one
 * wins — Tailwind's own, this design system's custom scales, and daisyUI's
 * component modifiers alike.
 */
export function cn(...inputs: ClassValue[]): string {
  return twMerge(clsx(inputs));
}

/** The font-size steps this package registers with `tailwind-merge`. Exported
 * for `theme-tokens.test.ts`, which checks them against `theme.css`. */
export const REGISTERED_FONT_SIZES: readonly string[] = FONT_SIZES;
