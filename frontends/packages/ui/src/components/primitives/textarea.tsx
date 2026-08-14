import type { TextareaHTMLAttributes } from "react";
import { forwardRef } from "react";
import { cn } from "../../lib/cn";

export type TextareaProps = TextareaHTMLAttributes<HTMLTextAreaElement>;

// D8: same reasoning as `input.tsx` — daisyUI's own `.textarea` rule sets
// `border-radius: var(--radius-field)` directly (confirmed by reading
// `daisyui/components/textarea.css`), so no `rounded-*` class is added
// here. The class string is unchanged; the token rewrite in Phase 0 is
// what moved the rendered radius.
export const Textarea = forwardRef<HTMLTextAreaElement, TextareaProps>(
  ({ className, ...props }, ref) => (
    <textarea
      ref={ref}
      className={cn("textarea textarea-bordered w-full font-sans text-prose", className)}
      {...props}
    />
  ),
);
Textarea.displayName = "Textarea";
