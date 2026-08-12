import type { SVGProps } from "react";
import { cn } from "../../lib/cn";
import {
  MESSAGE_STATUS_META,
  type MessageState,
  type StatusMark,
  type StatusMeta,
} from "./status-tokens";

export interface StateMarkProps extends Omit<SVGProps<SVGSVGElement>, "className"> {
  state: MessageState;
  /** Rendered size in px. Below 12 the glyph is not rendered (design doc §4.6). */
  size?: 12 | 14 | 16;
  className?: string;
}

export interface StateMarkFromMetaProps extends Omit<SVGProps<SVGSVGElement>, "className"> {
  meta: StatusMeta;
  size?: 12 | 14 | 16;
  className?: string | undefined;
}

// Geometry per design doc §4.3, at a 16x16 viewBox: circle r=6 centred at
// (8,8), stroke 1.5; the progress wedge/ring inset 1.5 from that stroke
// (r=4.5); diamond = square rotated 45deg, circumradius 7; square = 10x10
// centred, corner radius 1. All at stroke-width 1.5.
const CX = 8;
const CY = 8;
const R = 6;
const R_INNER = 4.5;

/** Clockwise pie wedge from 12 o'clock, `sweepDeg` degrees, radius `r`. */
function pieWedgePath(sweepDeg: number, r: number): string {
  const rad = (sweepDeg * Math.PI) / 180;
  const x = CX + r * Math.sin(rad);
  const y = CY - r * Math.cos(rad);
  const largeArc = sweepDeg > 180 ? 1 : 0;
  return `M${CX},${CY} L${CX},${CY - r} A${r},${r} 0 ${largeArc},1 ${round(x)},${round(y)} Z`;
}

function round(n: number): number {
  return Math.round(n * 100) / 100;
}

const DIAMOND_POINTS = `${CX},${CY - 7} ${CX + 7},${CY} ${CX},${CY + 7} ${CX - 7},${CY}`;

function Silhouette({ meta }: { meta: StatusMeta }) {
  const fill = meta.filled ? "currentColor" : "none";
  const stroke = "currentColor";

  switch (meta.silhouette) {
    case "circle":
      return <circle cx={CX} cy={CY} r={R} fill={fill} stroke={stroke} strokeWidth={1.5} />;
    case "diamond":
      return <polygon points={DIAMOND_POINTS} fill={fill} stroke={stroke} strokeWidth={1.5} />;
    case "square":
      return (
        <rect
          x={3}
          y={3}
          width={10}
          height={10}
          rx={1}
          fill={fill}
          stroke={stroke}
          strokeWidth={1.5}
        />
      );
  }
}

/**
 * Terminal states (delivered/cancelled/expired/rejected/failed) render a
 * solid, filled silhouette (design doc §4.3: "is it over?" is readable from
 * fill alone). The interior mark then has to cut through that fill to stay
 * legible — this is the one place `StateMark` is not literally
 * single-tone: the mark strokes in `--state-mark-knockout` (default: the
 * page canvas colour) rather than `currentColor`, since a currentColor
 * stroke on a currentColor fill would be invisible. Non-terminal marks
 * (pie wedges, the submitted ring, the uncertain "?", the undelivered
 * pause bars) stay `currentColor` throughout, on an unfilled silhouette.
 */
function InteriorMark({ mark, knockout }: { mark: StatusMark; knockout: boolean }) {
  const stroke = knockout ? "var(--state-mark-knockout, var(--color-base-100))" : "currentColor";
  const common = {
    stroke,
    strokeWidth: 1.5,
    strokeLinecap: "round" as const,
    fill: "none" as const,
  };

  switch (mark) {
    case "pie-1":
      return <path d={pieWedgePath(90, R_INNER)} fill="currentColor" />;
    case "pie-2":
      return <path d={pieWedgePath(180, R_INNER)} fill="currentColor" />;
    case "pie-3":
      return <path d={pieWedgePath(270, R_INNER)} fill="currentColor" />;
    case "ring":
      return (
        <circle cx={CX} cy={CY} r={R_INNER} fill="none" stroke="currentColor" strokeWidth={1.5} />
      );
    case "check":
      return <polyline points="5.4,8.3 7.2,10.2 11,5.8" strokeLinejoin="round" {...common} />;
    case "cross":
      return (
        <g {...common}>
          <line x1={5.6} y1={5.6} x2={10.4} y2={10.4} />
          <line x1={10.4} y1={5.6} x2={5.6} y2={10.4} />
        </g>
      );
    case "slash":
      return <line x1={5.6} y1={10.4} x2={10.4} y2={5.6} {...common} />;
    case "bar":
      return <line x1={5} y1={8} x2={11} y2={8} {...common} />;
    case "clock":
      return (
        <g {...common}>
          <line x1={8} y1={8} x2={8} y2={5.4} />
          <line x1={8} y1={8} x2={10.1} y2={8.9} />
        </g>
      );
    case "question":
      return (
        <g {...common}>
          <path d="M6.2 6.6c0-1.1 0.9-1.9 1.9-1.9s1.9 0.7 1.9 1.7c0 1.3-1.9 1.3-1.9 2.9" />
          <circle cx={8} cy={10.7} r={0.15} fill="currentColor" stroke="none" />
        </g>
      );
    case "pause":
      return (
        <g {...common}>
          <line x1={6.3} y1={5.5} x2={6.3} y2={10.5} />
          <line x1={9.7} y1={5.5} x2={9.7} y2={10.5} />
        </g>
      );
  }
}

/**
 * The same eleven-glyph geometry (design doc §5.3), parameterised on a raw
 * [`StatusMeta`] rather than a specific state enum — what [`StateMark`]
 * delegates to, and what a status pill for a *different* state machine
 * (e.g. `JobStatusPill`, #56) renders through instead of forking this
 * file's geometry. Pure: no state of its own, `aria-hidden` — the
 * accessible name lives on the wrapping pill component.
 */
export function StateMarkFromMeta({
  meta,
  size = 14,
  className,
  ...props
}: StateMarkFromMetaProps) {
  return (
    <svg
      viewBox="0 0 16 16"
      width={size}
      height={size}
      aria-hidden="true"
      className={cn("shrink-0", className)}
      {...props}
    >
      <Silhouette meta={meta} />
      <InteriorMark mark={meta.mark} knockout={meta.filled} />
    </svg>
  );
}

/**
 * The eleven-glyph SVG primitive (design doc §5.3), for `MessageState`
 * specifically. Every message status representation in the product
 * renders through this.
 */
export function StateMark({ state, size = 14, className, ...props }: StateMarkProps) {
  return (
    <StateMarkFromMeta
      meta={MESSAGE_STATUS_META[state]}
      size={size}
      className={className}
      {...props}
    />
  );
}
