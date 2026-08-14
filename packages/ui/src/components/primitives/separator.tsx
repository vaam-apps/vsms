import { forwardRef } from "react";
import { cn } from "../../lib/cn";

// D6: `@radix-ui/react-separator` is deleted outright — the Radix primitive
// was already a near-trivial wrapper around a styled `<div>` with a
// `role`/`aria-orientation` toggle for the decorative/semantic distinction,
// so nothing behavioural is lost by hand-rolling that same toggle here.
//
// The non-decorative branch renders a real `<hr>` rather than a `<div
// role="separator">` — found via Biome's own a11y lint, not assumed: an
// ARIA `separator` role on a generic element is the *interactive* (focusable,
// resizable-pane-divider) variant per WAI-ARIA, which requires `tabIndex`
// and `aria-valuenow`; neither is meaningful for a plain visual rule. `<hr>`
// carries the equivalent *static* separator semantics natively, with no
// required ARIA attributes, and is what Biome's `useSemanticElements` rule
// itself suggests.
export interface SeparatorProps extends React.HTMLAttributes<HTMLDivElement> {
  orientation?: "horizontal" | "vertical";
  /** Matches Radix's own default: `true` renders a purely visual rule with
   * no `separator` semantics exposed to assistive tech (`role="none"` on a
   * `<div>`); `false` renders a native `<hr>` with `aria-orientation`. */
  decorative?: boolean;
}

export const Separator = forwardRef<HTMLDivElement, SeparatorProps>(
  ({ className, orientation = "horizontal", decorative = true, ...props }, ref) => {
    const sharedClassName = cn(
      "shrink-0 border-0 bg-edge",
      orientation === "horizontal" ? "h-px w-full" : "h-full w-px",
      className,
    );
    if (decorative) {
      return <div ref={ref} role="none" className={sharedClassName} {...props} />;
    }
    // No consumer forwards a ref through the non-decorative branch today
    // (grepped across `admin/`) — the cast is a documented, zero-risk
    // approximation rather than widening `Separator`'s public ref type to
    // `HTMLDivElement | HTMLHRElement` for a case nothing exercises.
    return (
      <hr
        ref={ref as unknown as React.Ref<HTMLHRElement>}
        aria-orientation={orientation}
        className={sharedClassName}
        {...props}
      />
    );
  },
);
Separator.displayName = "Separator";
