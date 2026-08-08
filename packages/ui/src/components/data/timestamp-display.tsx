"use client";

import { useEffect, useState, useSyncExternalStore } from "react";
import { cn } from "../../lib/cn";

// A single shared 30s interval drives every `TimestampDisplay` instance's
// relative-time re-render (design doc §7.2: "a table of 200 rows must not
// run 200 timers"). Lazily started on the first mounted instance, torn
// down when the last one unmounts.

let tick = 0;
const listeners = new Set<() => void>();
let timer: ReturnType<typeof setInterval> | null = null;

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  timer ??= setInterval(() => {
    tick += 1;
    for (const l of listeners) l();
  }, 30_000);
  return () => {
    listeners.delete(listener);
    if (listeners.size === 0 && timer !== null) {
      clearInterval(timer);
      timer = null;
    }
  };
}

function useSharedTick(): number {
  return useSyncExternalStore(
    subscribe,
    () => tick,
    () => 0,
  );
}

function pad(n: number): string {
  return String(n).padStart(2, "0");
}

/** `2026-08-08 14:03:07` + a zone suffix — ISO-ordered so it sorts and
 * compares visually (design doc §7.2). Only UTC is implemented (`Z`
 * suffix, always): the Africa/Douala toggle lives on the top bar, which
 * this screen doesn't build. "A bare local time with no zone label never
 * appears anywhere in this product" still holds — the suffix is always
 * present, just not yet switchable. */
function formatAbsolute(date: Date): string {
  const y = date.getUTCFullYear();
  const mo = pad(date.getUTCMonth() + 1);
  const d = pad(date.getUTCDate());
  const h = pad(date.getUTCHours());
  const mi = pad(date.getUTCMinutes());
  const s = pad(date.getUTCSeconds());
  return `${y}-${mo}-${d} ${h}:${mi}:${s}Z`;
}

function formatRelative(diffMs: number): string {
  const minutes = Math.floor(diffMs / 60_000);
  if (minutes < 1) return "just now";
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  return `${hours}h`;
}

export interface TimestampDisplayProps {
  /** ISO-8601. */
  value: string;
  className?: string;
}

/**
 * Relative for anything under 24h (`2m`, `47m`, `6h`), absolute otherwise
 * — design doc §7.2. Renders the absolute value on the server and on
 * first client render (so server/client markup matches and Next doesn't
 * warn/flash), then upgrades to relative once mounted, driven by the
 * shared interval above rather than its own timer.
 */
export function TimestampDisplay({ value, className }: TimestampDisplayProps) {
  useSharedTick();
  const [hydrated, setHydrated] = useState(false);
  useEffect(() => setHydrated(true), []);

  const date = new Date(value);
  const absolute = formatAbsolute(date);

  if (!hydrated) {
    return (
      <span className={cn("font-mono text-subtle-foreground tabular-nums", className)}>
        {absolute}
      </span>
    );
  }

  const diffMs = Date.now() - date.getTime();
  const display = diffMs >= 0 && diffMs < 24 * 3600 * 1000 ? formatRelative(diffMs) : absolute;

  return (
    <span
      title={absolute}
      className={cn("font-mono text-subtle-foreground tabular-nums", className)}
    >
      {display}
    </span>
  );
}
