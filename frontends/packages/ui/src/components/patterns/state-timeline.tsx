import { Info } from "lucide-react";
import type { ReactNode } from "react";
import { cn } from "../../lib/cn";
import { Skeleton } from "../primitives/skeleton";
import { StateMark } from "../status/state-mark";
import type { StatusSystem } from "../status/status-tokens";
import { type PayloadExchange, PayloadInspector } from "./payload-inspector";

export interface StateTransition<S extends string = string> {
  toState: S;
  /** ISO 8601 timestamp. */
  at: string;
  actor?: string;
  providerKey?: string;
  workerNode?: string;
  attempt?: number;
  maxAttempts?: number;
  payload?: PayloadExchange[];
}

export interface StateTimelineProps<S extends string = string> {
  transitions: StateTransition<S>[];
  /** The state machine's presentation table — the same one its
   * `createStatusPill` is bound to, so a timeline and a pill can never
   * disagree about what a state looks like. */
  system: StatusSystem<S>;
  currentState: S;
  /** Whether the timeline has stopped. Passed in rather than derived from
   * `system[currentState].family`: terminality is the server's fact, and
   * a presentational table is the wrong place to learn it from — see
   * `status-tokens.ts`'s own warning. */
  isTerminal: boolean;
  /**
   * Any IANA zone name, e.g. `"UTC"`, `"Africa/Douala"`, `"America/New_York"`.
   * The UTC-offset suffix is derived from the zone and the instant, so it
   * stays correct across DST and for zones this library has never heard
   * of. (It used to be a two-value union with the suffix hard-coded as
   * `"Z"` or `"+01"` — correct only for the two zones that union named.)
   */
  timezone?: string;
  /**
   * Per-state explanatory notes, rendered beneath the node that entered
   * that state.
   *
   * The point is states that look like bugs to anyone who does not
   * already know the product decision behind them — "we never learned the
   * outcome, and deliberately will not retry". Without the note the
   * operator's next move is to open a SQL client, which is the outcome a
   * timeline exists to prevent. Supplied by the caller because the
   * explanation is domain knowledge, not presentation.
   */
  annotations?: Partial<Record<S, string>> | undefined;
}

/**
 * `2026-09-11 14:03:07 +01` — sortable, unambiguous, and carrying its own
 * offset so a screenshot pasted into a ticket is still interpretable.
 *
 * The offset is read out of `Intl`'s own `shortOffset` part rather than
 * assumed from the zone name: it is the only way to be right across DST
 * and across zones this component has never been told about. `UTC` keeps
 * its conventional `Z` rather than the `GMT` that `shortOffset` yields.
 */
function formatAbsolute(iso: string, timezone: string): string {
  const date = new Date(iso);
  const formatter = new Intl.DateTimeFormat("en-CA", {
    timeZone: timezone,
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    hourCycle: "h23",
    timeZoneName: "shortOffset",
  });
  const parts = formatter.formatToParts(date);
  const stamp = parts
    .filter((part) => part.type !== "timeZoneName" && part.type !== "literal")
    .reduce<string[]>((acc, part) => {
      acc.push(part.value);
      return acc;
    }, []);
  const [year, month, day, hour, minute, second] = stamp;
  // "GMT+1" / "GMT-05:00" / "GMT" → "+01" / "-05:00" / "Z".
  const raw = parts.find((part) => part.type === "timeZoneName")?.value ?? "";
  const offset = raw === "GMT" ? "Z" : raw.replace(/^GMT/, "").replace(/^([+-])(\d)$/, "$10$2");
  return `${year}-${month}-${day} ${hour}:${minute}:${second} ${offset}`;
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
 * A record's transition history: one node per state entered, with elapsed
 * time between them, optional per-transition metadata, and an optional
 * payload inspector.
 *
 * Generic over the state machine — pass the same [`StatusSystem`] its
 * pill is bound to. It used to be hard-wired to one application's message
 * states, including that application's own explanatory notes and a
 * two-value timezone union; all three are now the caller's.
 */
export function StateTimeline<S extends string>({
  transitions,
  system,
  currentState,
  isTerminal,
  timezone = "UTC",
  annotations,
}: StateTimelineProps<S>) {
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
    const meta = system[transition.toState];
    const isLast = i === transitions.length - 1;

    rows.push(
      <li
        key={`${transition.toState}-${transition.at}`}
        className="relative flex gap-3 pb-6 last:pb-0"
      >
        <div className="flex w-4 shrink-0 flex-col items-center">
          <StateMark meta={meta} size={16} className="text-foreground" />
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

    const annotation = annotations?.[transition.toState];
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
          <StateMark meta={system[currentState]} size={12} className="text-muted-foreground" />
          <span>still moving</span>
        </div>
      </li>,
    );
  }

  return <ol className={cn("flex flex-col")}>{rows}</ol>;
}
