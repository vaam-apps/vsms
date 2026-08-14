import { Info } from "lucide-react";
import type { ReactNode } from "react";
import { cn } from "../../lib/cn";
import { Skeleton } from "../primitives/skeleton";
import { StateMark } from "../status/state-mark";
import { MESSAGE_STATUS_META, type MessageState } from "../status/status-tokens";
import { type PayloadExchange, PayloadInspector } from "./payload-inspector";

export interface StateTransition {
  toState: MessageState;
  /** ISO 8601 timestamp. */
  at: string;
  actor?: string;
  providerKey?: string;
  workerNode?: string;
  attempt?: number;
  maxAttempts?: number;
  payload?: PayloadExchange[];
}

export interface StateTimelineProps {
  transitions: StateTransition[];
  currentState: MessageState;
  isTerminal: boolean;
  timezone?: "UTC" | "Africa/Douala";
}

function formatAbsolute(iso: string, timezone: "UTC" | "Africa/Douala"): string {
  const date = new Date(iso);
  const tz = timezone === "UTC" ? "UTC" : "Africa/Douala";
  const formatted = new Intl.DateTimeFormat("en-CA", {
    timeZone: tz,
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    hourCycle: "h23",
  })
    .format(date)
    .replace(",", "");
  const suffix = timezone === "UTC" ? "Z" : "+01";
  return `${formatted} ${suffix}`;
}

function formatElapsed(ms: number): string {
  if (ms < 1000) return `+${ms}ms`;
  if (ms < 60_000) return `+${(ms / 1000).toFixed(3)}s`;
  const totalSeconds = Math.round(ms / 1000);
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = totalSeconds % 60;
  if (minutes < 60) return `+${minutes}m ${String(seconds).padStart(2, "0")}s`;
  const hours = Math.floor(minutes / 60);
  return `+${hours}h ${String(minutes % 60).padStart(2, "0")}m`;
}

/**
 * The two states that look exactly like bugs to anyone who doesn't already
 * know the product decision behind them (design doc §5.3, quoting §4.7
 * verbatim). Without this annotation, the operator's next move is to open
 * psql — precisely the outcome the epic gate (#45/#50) forbids.
 */
const ANNOTATIONS: Partial<Record<MessageState, string>> = {
  uncertain:
    "The outcome was never learned. providerMessageRefAlt was stamped with the message id so a late DLR can still correlate. This message will not be resubmitted — a deliberate trade against sending a duplicate OTP.",
  undelivered:
    'The provider said "not delivered", not "never". undelivered -> queued is a legal edge, but no retry driver runs today (#122) — this message will stay here until someone acts.',
};

function AnnotationNode({ text }: { text: string }) {
  return (
    <li className="relative flex gap-3 pb-4 pl-0">
      <div className="flex w-4 shrink-0 justify-center">
        <Info size={14} strokeWidth={1.5} className="mt-0.5 text-muted-foreground" />
      </div>
      <div className="min-w-0 flex-1 rounded-sm border border-edge bg-surface-2 px-3 py-2 text-caption text-muted-foreground">
        {text}
      </div>
    </li>
  );
}

/**
 * The message detail's transition history (design doc §5.3) — the epic
 * gate's own component: "an operator can diagnose a failed message
 * without touching SQL."
 */
export function StateTimeline({
  transitions,
  currentState,
  isTerminal,
  timezone = "UTC",
}: StateTimelineProps) {
  if (transitions.length === 0) {
    return (
      <ol className="flex flex-col gap-0">
        {[0, 1, 2].map((i) => (
          <li key={i} className="flex items-center gap-3 py-2">
            <Skeleton className="h-4 w-4 rounded-full" />
            <Skeleton className="h-4 w-40" />
          </li>
        ))}
      </ol>
    );
  }

  const rows: ReactNode[] = [];

  transitions.forEach((transition, i) => {
    const previous = transitions[i - 1];
    const elapsedMs = previous
      ? new Date(transition.at).getTime() - new Date(previous.at).getTime()
      : null;
    const meta = MESSAGE_STATUS_META[transition.toState];
    const isLast = i === transitions.length - 1;

    rows.push(
      <li
        key={`${transition.toState}-${transition.at}`}
        className="relative flex gap-3 pb-6 last:pb-0"
      >
        <div className="flex w-4 shrink-0 flex-col items-center">
          <StateMark state={transition.toState} size={16} className="text-foreground" />
          {!isLast && <span className="mt-1 w-px flex-1 bg-[var(--state-mark-rail,var(--edge))]" />}
        </div>
        <div className="min-w-0 flex-1">
          <div className="flex items-baseline justify-between gap-2">
            <p className="font-medium text-body text-foreground">
              {meta.label}{" "}
              <span className="font-mono text-subtle-foreground">{transition.toState}</span>
            </p>
            <div className="shrink-0 text-right">
              <p className="font-mono text-caption text-foreground">
                {formatAbsolute(transition.at, timezone)}
              </p>
              {elapsedMs != null && (
                <p className="font-mono text-caption text-subtle-foreground">
                  {formatElapsed(elapsedMs)}
                </p>
              )}
            </div>
          </div>
          {(transition.providerKey || transition.workerNode || transition.attempt) != null && (
            <p className="mt-1 font-mono text-caption text-subtle-foreground">
              {[
                transition.providerKey,
                transition.workerNode,
                transition.attempt != null
                  ? `attempt ${transition.attempt}${transition.maxAttempts ? `/${transition.maxAttempts}` : ""}`
                  : null,
              ]
                .filter(Boolean)
                .join(" · ")}
            </p>
          )}
          {transition.payload != null && transition.payload.length > 0 && (
            <div className="mt-2">
              <PayloadInspector exchanges={transition.payload} defaultOpen={-1} />
            </div>
          )}
        </div>
      </li>,
    );

    const annotation = ANNOTATIONS[transition.toState];
    if (annotation != null) {
      rows.push(<AnnotationNode key={`${transition.toState}-annotation`} text={annotation} />);
    }
  });

  // In-flight cap (design doc §5.3): the rail continues past the last node
  // as a dashed segment ending in the current-state glyph, so "still
  // moving" is readable without parsing. A terminal timeline instead ends
  // the last node's own rail segment (no trailing cap at all).
  if (!isTerminal) {
    rows.push(
      <li key="in-flight-cap" className="relative flex gap-3">
        <div className="flex w-4 shrink-0 justify-center">
          <span className="h-6 w-px border-edge-strong border-l border-dashed" aria-hidden="true" />
        </div>
        <div className="flex items-center gap-2 text-caption text-subtle-foreground">
          <StateMark state={currentState} size={12} className="text-muted-foreground" />
          <span>still moving</span>
        </div>
      </li>,
    );
  }

  return <ol className={cn("flex flex-col")}>{rows}</ol>;
}
