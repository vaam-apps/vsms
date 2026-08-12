import type { ReactNode } from "react";
import { cn } from "../../lib/cn";
import { StateMark } from "./state-mark";
import { MESSAGE_STATUS_META, type MessageState, type StatusHue } from "./status-tokens";

/** Exported for `JobStatusPill` (#56) — same hue→class mapping, reused
 * rather than duplicated. */
export const HUE_CLASSES: Record<StatusHue, { fg: string; bg: string; border: string }> = {
  neutral: {
    fg: "text-state-neutral-fg",
    bg: "bg-state-neutral-bg",
    border: "border-state-neutral-border",
  },
  success: {
    fg: "text-state-success-fg",
    bg: "bg-state-success-bg",
    border: "border-state-success-border",
  },
  danger: {
    fg: "text-state-danger-fg",
    bg: "bg-state-danger-bg",
    border: "border-state-danger-border",
  },
  uncertain: {
    fg: "text-state-uncertain-fg",
    bg: "bg-state-uncertain-bg",
    border: "border-state-uncertain-border",
  },
  expired: {
    fg: "text-state-expired-fg",
    bg: "bg-state-expired-bg",
    border: "border-state-expired-border",
  },
  parked: {
    fg: "text-state-parked-fg",
    bg: "bg-state-parked-bg",
    border: "border-state-parked-border",
  },
};

export interface StatusPillProps {
  state: MessageState;
  /** Default `'auto'`: resolved from the design doc's attention ladder (§4.5). */
  variant?: "auto" | "quiet" | "loud";
  size?: "sm" | "md";
  /** Render the mono enum literal beside the localised label. True in detail views, false in tables. */
  showLiteral?: boolean;
  locale?: "en" | "fr";
  /** Short mono qualifier, e.g. `failed · 4xx`. */
  detail?: ReactNode;
  /** Optimistic-transition case: dimmed + dashed, held until the server confirms (design doc §5.3). */
  pending?: boolean;
  interactive?: boolean;
  onClick?: () => void;
  className?: string;
}

/**
 * The canonical status representation (design doc §5.3): glyph + label +
 * attention treatment. This is the single most-reused component in the
 * console — every screen that shows a message state renders through it.
 */
export function StatusPill({
  state,
  variant = "auto",
  size = "sm",
  showLiteral = false,
  locale = "en",
  detail,
  pending = false,
  interactive = false,
  onClick,
  className,
}: StatusPillProps) {
  const meta = MESSAGE_STATUS_META[state];
  const resolvedVariant = variant === "auto" ? meta.attention : variant;
  const loud = resolvedVariant === "loud";
  const hue = HUE_CLASSES[meta.hue];
  const label = locale === "fr" ? meta.labelFr : meta.labelEn;
  const markSize = size === "md" ? 16 : 14;

  const Comp = interactive ? "button" : "span";

  return (
    <Comp
      type={interactive ? "button" : undefined}
      onClick={interactive ? onClick : undefined}
      title={meta.tooltipEn}
      aria-label={`${state} — ${label}`}
      className={cn(
        "inline-flex items-center gap-[5px] whitespace-nowrap align-middle",
        "text-caption",
        loud && [hue.bg, hue.border, "rounded-sm border py-0.5 pr-1.5 pl-[5px] font-medium"],
        !loud && "gap-[5px]",
        pending && "border-dashed opacity-60",
        interactive && "cursor-pointer",
        className,
      )}
    >
      <StateMark state={state} size={markSize} className={hue.fg} />
      <span className={loud ? hue.fg : "text-muted-foreground"}>{label}</span>
      {showLiteral && <span className="font-mono text-subtle-foreground text-[11px]">{state}</span>}
      {detail != null && <span className="font-mono text-subtle-foreground">{detail}</span>}
    </Comp>
  );
}
