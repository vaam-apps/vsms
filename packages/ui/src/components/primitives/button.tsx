import { Slot } from "@radix-ui/react-slot";
import type { ButtonHTMLAttributes } from "react";
import { forwardRef } from "react";
import { cn } from "../../lib/cn";

export type ButtonVariant = "primary" | "secondary" | "ghost" | "destructive";
export type ButtonSize = "sm" | "md" | "icon";

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: ButtonVariant;
  size?: ButtonSize;
  /** Render as the single child element instead of a `<button>` (Radix `Slot`). */
  asChild?: boolean;
}

// daisyUI does the heavy lifting (`btn-primary`, `btn-outline`, `btn-ghost`,
// `btn-error`); the design doc's rule (§5.1) is that there is no
// "success"/"warning" button variant — those hues are reserved for status,
// so only these four exist. `btn-primary` is genuinely the achromatic
// inverse fill (§1.3: near-black on light, near-white on dark) — `neutral`
// is a *different*, deliberately quieter token (this theme's surface-3-ish
// fill, used for badges) and using it here read as a washed-out primary
// button in light theme; caught visually, not by inspection.
const VARIANT_CLASSES: Record<ButtonVariant, string> = {
  primary: "btn-primary",
  secondary: "btn-outline",
  ghost: "btn-ghost",
  destructive: "btn-error",
};

const SIZE_CLASSES: Record<ButtonSize, string> = {
  sm: "btn-sm",
  md: "",
  icon: "btn-square btn-sm",
};

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(
  ({ className, variant = "primary", size = "md", asChild = false, ...props }, ref) => {
    const Comp = asChild ? Slot : "button";
    return (
      <Comp
        ref={ref}
        className={cn(
          "btn font-sans font-semibold",
          VARIANT_CLASSES[variant],
          SIZE_CLASSES[size],
          className,
        )}
        {...props}
      />
    );
  },
);
Button.displayName = "Button";
