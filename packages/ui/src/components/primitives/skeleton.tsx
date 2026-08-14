import type { HTMLAttributes } from "react";
import { cn } from "../../lib/cn";

/**
 * Static — never shimmering (design doc §3.8 rule 2: "no skeleton
 * shimmer"; §5.2: skeleton rows must match the real row height exactly so
 * nothing shifts on load). House rule, not negotiable — see this repo's
 * own AGENTS.md-lineage note; do not add animation here.
 *
 * Deliberately hand-rolled `<div>`, not daisyUI's own `.skeleton` class:
 * read `daisyui/components/skeleton.css` directly before deciding this —
 * daisyUI's `.skeleton` ships a `background-position` shimmer under
 * `animation:1.8s ease-in-out infinite skeleton` (guarded only by
 * `prefers-reduced-motion`, which is not the same thing as "off by
 * default"), which is exactly the animation this house rule forbids.
 * Radius stays `rounded-sm` (now `--radius-field`, 12px, via Phase 0's
 * token rewrite) rather than moving to `--radius-box` — this component
 * stands in for table cells and text lines across every screen, all
 * field-scale content, not card-scale surfaces, so the token that already
 * existed here is also the correct D8 tier, not a value left untouched by
 * omission.
 */
export function Skeleton({ className, ...props }: HTMLAttributes<HTMLDivElement>) {
  return <div className={cn("rounded-sm bg-surface-3", className)} {...props} />;
}
