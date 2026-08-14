import { cva, type VariantProps } from "class-variance-authority";
import type { ButtonHTMLAttributes } from "react";
import { forwardRef } from "react";
import { cn } from "../../lib/cn";

// daisyUI does the heavy lifting (`btn-primary`, `btn-outline`, `btn-ghost`,
// `btn-error`); the design doc's rule (§5.1) is that there is no
// "success"/"warning" button variant — those hues are reserved for status,
// so only these four exist. `btn-primary` is genuinely the achromatic
// inverse fill (§1.3: near-black on light, near-white on dark) — `neutral`
// is a *different*, deliberately quieter token (this theme's surface-3-ish
// fill, used for badges) and using it here read as a washed-out primary
// button in light theme; caught visually, not by inspection.
//
// D4/D11: rebuilt on `cva()` per docs/design/console-redesign.md §6.3 —
// `Slot`/`asChild` are gone (nothing in this repo used `<Button asChild>`,
// confirmed by grep before removing it), and `buttonVariants` is exported
// standalone so a link that must look like a button reaches for the class
// string directly: `<a className={buttonVariants({ variant: "secondary" })}>`.
// `rounded-field` in the base string is the one deliberate D8 addition —
// daisyUI's own `.btn` class already applies `--radius-field` per corner
// internally, so this doesn't change the rendered radius, only makes the
// D8 register explicit at the call site, matching §6.3's own sketch
// verbatim.
export const buttonVariants = cva("btn font-sans font-semibold rounded-field", {
  variants: {
    variant: {
      primary: "btn-primary",
      secondary: "btn-outline",
      ghost: "btn-ghost",
      destructive: "btn-error",
    },
    size: {
      sm: "btn-sm",
      md: "",
      icon: "btn-square btn-sm",
    },
  },
  defaultVariants: { variant: "primary", size: "md" },
});

export type ButtonVariant = NonNullable<VariantProps<typeof buttonVariants>["variant"]>;
export type ButtonSize = NonNullable<VariantProps<typeof buttonVariants>["size"]>;

export interface ButtonProps
  extends ButtonHTMLAttributes<HTMLButtonElement>,
    VariantProps<typeof buttonVariants> {}

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(
  ({ className, variant, size, ...props }, ref) => (
    <button ref={ref} className={cn(buttonVariants({ variant, size }), className)} {...props} />
  ),
);
Button.displayName = "Button";
