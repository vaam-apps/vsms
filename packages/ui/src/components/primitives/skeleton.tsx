import type { HTMLAttributes } from "react";
import { cn } from "../../lib/cn";

/**
 * Static — never shimmering (design doc §3.8 rule 2: "no skeleton
 * shimmer"; §5.2: skeleton rows must match the real row height exactly so
 * nothing shifts on load).
 */
export function Skeleton({ className, ...props }: HTMLAttributes<HTMLDivElement>) {
  return <div className={cn("rounded-sm bg-surface-3", className)} {...props} />;
}
