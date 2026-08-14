import type { InputHTMLAttributes } from "react";
import { forwardRef } from "react";
import { cn } from "../../lib/cn";

export type InputProps = InputHTMLAttributes<HTMLInputElement>;

// D8: no explicit `rounded-*` class needed or added — daisyUI's own
// `.input` rule already sets every corner from `--radius-field`
// (confirmed by reading `daisyui/components/input.css` directly), and
// Phase 0 already rewrote that token to the new register. The class
// string here is unchanged from before the redesign; the rendered radius
// changed anyway, entirely from the token, with zero component code to
// touch — the intended effect of constraint 7 ("DaisyUI does the work").
export const Input = forwardRef<HTMLInputElement, InputProps>(({ className, ...props }, ref) => (
  <input
    ref={ref}
    className={cn("input input-bordered w-full font-sans text-prose", className)}
    {...props}
  />
));
Input.displayName = "Input";
